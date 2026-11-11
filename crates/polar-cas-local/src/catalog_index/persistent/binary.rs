//! Portable typed position records. The local section comes first so playback
//! lookup never decodes Store descriptors or writer attribution.
use super::{IndexedLocation, Positions, SelectedLocation};
use polar_cas::{
    catalog::writers::{SaveAttribution, WriterId, WriterRecord},
    shared::{LocatedContent, PhysicalCandidate},
    source::{LocationId, SourceLocator},
    store_index::{Representation, StoreEntry, StoreRef},
    ContentRef, DigestAlgorithm, Error, ObjectId, Result,
};

pub(in crate::catalog_index) fn key(id: &ObjectId) -> [u8; 33] {
    let mut out = [0; 33];
    out[..32].copy_from_slice(id.digest_bytes());
    out[32] = u8::from(id.algorithm() == DigestAlgorithm::Blake3);
    out
}
struct Encoder(Vec<u8>);
impl Encoder {
    fn byte(&mut self, value: u8) {
        self.0.push(value);
    }
    fn number(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }
    fn count(&mut self, value: usize) -> Result<()> {
        let value = u32::try_from(value).map_err(|_| Error::TooLarge)?;
        self.0.extend_from_slice(&value.to_le_bytes());
        Ok(())
    }
    fn string(&mut self, value: &str) -> Result<()> {
        self.count(value.len())?;
        self.0.extend_from_slice(value.as_bytes());
        Ok(())
    }
    fn object(&mut self, value: &ObjectId) {
        self.0.extend_from_slice(&key(value));
    }
    fn content(&mut self, value: &ContentRef) {
        self.object(&value.id);
        self.number(value.size_bytes);
    }
    fn optional<T>(
        &mut self,
        value: Option<&T>,
        write: impl FnOnce(&mut Self, &T) -> Result<()>,
    ) -> Result<()> {
        self.byte(u8::from(value.is_some()));
        if let Some(value) = value {
            write(self, value)?;
        }
        Ok(())
    }
    fn scope(&mut self, value: Option<&ObjectId>) -> Result<()> {
        self.optional(value, |w, id| {
            w.object(id);
            Ok(())
        })
    }
    fn store(&mut self, value: &StoreRef) -> Result<()> {
        self.0.extend_from_slice(value.uuid.as_bytes());
        self.content(&value.definition);
        self.string(&value.creator_node_id)
    }
}
struct Decoder<'a>(&'a [u8]);

impl<'a> Decoder<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let out = self.0.get(..n).ok_or(Error::Integrity)?;
        self.0 = &self.0[n..];
        Ok(out)
    }
    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn number(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().map_err(|_| Error::Integrity)?,
        ))
    }
    fn count(&mut self) -> Result<usize> {
        let count =
            u32::from_le_bytes(self.take(4)?.try_into().map_err(|_| Error::Integrity)?) as usize;
        // Every element consumes at least one byte. Reject malicious counts
        // before allocating, including when this is a string length.
        if count > self.0.len() {
            return Err(Error::Integrity);
        }
        Ok(count)
    }
    fn string(&mut self) -> Result<String> {
        let n = self.count()?;
        Ok(std::str::from_utf8(self.take(n)?)
            .map_err(|_| Error::Integrity)?
            .to_owned())
    }
    fn object(&mut self) -> Result<ObjectId> {
        let digest = self.take(32)?.try_into().map_err(|_| Error::Integrity)?;
        let algorithm = match self.byte()? {
            0 => DigestAlgorithm::Sha256,
            1 => DigestAlgorithm::Blake3,
            _ => return Err(Error::Integrity),
        };
        Ok(ObjectId::from_digest(algorithm, digest))
    }
    fn content(&mut self) -> Result<ContentRef> {
        ;
        Ok(ContentRef {
            id: self.object()?,
            size_bytes: self.number()?,
        })
    }
    fn optional<T>(&mut self, read: impl FnOnce(&mut Self) -> Result<T>) -> Result<Option<T>> {
        match self.byte()? {
            0 => Ok(None),
            1 => read(self).map(Some),
            _ => Err(Error::Integrity),
        }
    }
    fn store(&mut self) -> Result<StoreRef> {
        let uuid: [u8; 16] = self.take(16)?.try_into().map_err(|_| Error::Integrity)?;
        let value = StoreRef {
            uuid: uuid::Uuid::from_bytes(uuid),
            definition: self.content()?,
            creator_node_id: self.string()?,
        };
        value.validate()?;
        Ok(value)
    }
    fn local(&mut self) -> Result<LocatedContent> {
        let content = self.content()?;
        let location = match self.byte()? {
            0 => PhysicalCandidate::Loose,
            1 => PhysicalCandidate::PackMember {
                pack: self.content()?,
            },
            _ => return Err(Error::Integrity),
        };
        Ok(LocatedContent { content, location })
    }
    fn locals(&mut self) -> Result<Vec<LocatedContent>> {
        let count = self.count()?;
        (0..count).map(|_| self.local()).collect()
    }
}
fn reader(bytes: &[u8]) -> Result<Decoder<'_>> {
    if bytes.starts_with(b"PCASPOS2") {
        let offset = extension_offset(bytes)?;
        return Ok(Decoder(&bytes[8..offset]));
    }
    if !bytes.starts_with(b"PCASPOS1") {
        return Err(Error::Integrity);
    }
    Ok(Decoder(&bytes[8..]))
}
pub(super) fn first_local(bytes: &[u8]) -> Result<Option<LocatedContent>> {
    let mut r = reader(bytes)?;
    if r.count()? == 0 {
        Ok(None)
    } else {
        r.local().map(Some)
    }
}
pub(super) fn local(bytes: &[u8]) -> Result<Vec<LocatedContent>> {
    reader(bytes)?.locals()
}
pub(super) fn encode(value: &Positions) -> Result<Vec<u8>> {
    if value.extensions.len() > 1024 {
        return Err(Error::TooLarge);
    }
    if value.local.len() == 1
        && value.remote.is_empty()
        && value.writers.is_empty()
        && value.extensions.is_empty()
    {
        return crate::shared_index::position::single_local_value(&value.local[0]);
    }
    let mut w = Encoder(b"PCASPOS1".to_vec());
    w.count(value.local.len())?;
    for local in &value.local {
        w.content(&local.content);
        match &local.location {
            PhysicalCandidate::Loose => w.byte(0),
            PhysicalCandidate::PackMember { pack } => {
                w.byte(1);
                w.content(pack);
            }
        }
    }
    w.count(value.remote.len())?;
    for selected in &value.remote {
        w.scope(selected.scope.as_ref())?;
        let location = &selected.location;
        w.store(&location.store)?;
        w.content(&location.index_root);
        w.number(location.generation);
        w.number(location.received_at_ms);
        w.content(&location.entry.content);
        w.string(&location.entry.locator.key)?;
        w.optional(location.entry.locator.revision.as_ref(), |w, s| w.string(s))?;
        match &location.entry.representation {
            Representation::Blob => w.byte(0),
            Representation::DigestPackMember { image } => {
                w.byte(1);
                w.content(image);
            }
            Representation::FilesystemErofsMember { image, tree, path } => {
                w.byte(2);
                w.content(image);
                w.content(tree);
                w.string(path)?;
            }
        }
    }
    w.count(value.writers.len())?;
    for row in &value.writers {
        w.content(&row.content);
        w.string(row.attribution.writer.as_str())?;
        w.optional(row.attribution.store.as_ref(), Encoder::store)?;
        w.scope(row.attribution.scope.as_ref())?;
        w.string(row.destination.as_str())?;
        w.number(row.first_saved_at_ms);
    }
    if !value.extensions.is_empty() {
        w.0[..8].copy_from_slice(b"PCASPOS2");
        let offset = w.0.len() as u64;
        w.count(value.extensions.len())?;
        let mut previous = None;
        for extension in &value.extensions {
            if previous.is_some_and(|p| p >= extension.context) || extension.value.is_empty() {
                return Err(Error::Integrity);
            }
            previous = Some(extension.context);
            w.0.extend_from_slice(&extension.context);
            w.count(extension.value.len())?;
            w.0.extend_from_slice(&extension.value);
        }
        w.number(offset);
        if w.0.len() > 8 << 20 {
            return Err(Error::TooLarge);
        }
    }
    Ok(w.0)
}
pub(super) fn decode(bytes: &[u8]) -> Result<Positions> {
    let mut r = reader(bytes)?;
    let local = r.locals()?;
    let remote_count = r.count()?;
    let mut remote = Vec::new();
    for _ in 0..remote_count {
        let scope = r.optional(Decoder::object)?;
        let store = r.store()?;
        let index_root = r.content()?;
        let generation = r.number()?;
        let received_at_ms = r.number()?;
        let content = r.content()?;
        let locator = SourceLocator {
            key: r.string()?,
            revision: r.optional(Decoder::string)?,
        };
        let representation = match r.byte()? {
            0 => Representation::Blob,
            1 => Representation::DigestPackMember {
                image: r.content()?,
            },
            2 => Representation::FilesystemErofsMember {
                image: r.content()?,
                tree: r.content()?,
                path: r.string()?,
            },
            _ => return Err(Error::Integrity),
        };
        let entry = StoreEntry {
            content,
            locator,
            representation,
        };
        entry.validate()?;
        remote.push(SelectedLocation {
            scope,
            location: IndexedLocation {
                scope: None,
                store,
                index_root,
                generation,
                received_at_ms,
                entry,
            },
        });
    }
    let writer_count = r.count()?;
    let mut writers = Vec::new();
    for _ in 0..writer_count {
        let content = r.content()?;
        let writer = WriterId::new(r.string()?)?;
        let store = r.optional(Decoder::store)?;
        let scope = r.optional(Decoder::object)?;
        let destination = LocationId::new(r.string()?)?;
        let first_saved_at_ms = r.number()?;
        let row = WriterRecord {
            content,
            attribution: SaveAttribution {
                writer,
                store,
                scope,
            },
            destination,
            first_saved_at_ms,
        };
        row.validate()?;
        writers.push(row);
    }
    if !r.0.is_empty() {
        return Err(Error::Integrity);
    }
    let extensions = extension_views(bytes)?
        .into_iter()
        .map(
            |(context, value)| super::super::extensions::ScopedExtension {
                context,
                value: value.to_vec(),
            },
        )
        .collect();
    Ok(Positions {
        extensions,
        local,
        remote,
        writers,
    })
}
pub(in crate::catalog_index) fn encode_contents(contents: &[ContentRef]) -> Result<Vec<u8>> {
    let mut w = Encoder(b"PCASWRL1".to_vec());
    w.count(contents.len())?;
    for content in contents {
        w.content(content);
    }
    Ok(w.0)
}
pub(in crate::catalog_index) fn decode_contents(bytes: &[u8]) -> Result<Vec<ContentRef>> {
    if !bytes.starts_with(b"PCASWRL1") {
        return serde_json::from_slice(bytes).map_err(|_| Error::Integrity);
    }
    let list = ContentList::open(bytes)?;
    (0..list.len()).map(|n| list.at(n)).collect()
}

/// Borrow the already validated ordered list. The opening/restore boundary
/// checks every member; pagination reads O(log N + limit) fixed records.
struct ContentList<'a>(&'a [u8]);
impl<'a> ContentList<'a> {
    fn open(bytes: &'a [u8]) -> Result<Self> {
        let mut r = Decoder(bytes.strip_prefix(b"PCASWRL1").ok_or(Error::Integrity)?);
        let count = r.count()?;
        if count > 65_536 || r.0.len() != count * 41 {
            return Err(Error::Integrity);
        }
        Ok(Self(r.0))
    }
    fn len(&self) -> usize {
        self.0.len() / 41
    }
    fn at(&self, n: usize) -> Result<ContentRef> {
        let at = n.checked_mul(41).ok_or(Error::Integrity)?;
        Decoder(self.0.get(at..at + 41).ok_or(Error::Integrity)?).content()
    }
    fn after(&self, id: &ObjectId) -> Result<usize> {
        let (mut low, mut high) = (0, self.len());
        while low < high {
            let mid = low + (high - low) / 2;
            let row = self.at(mid)?;
            // Preserve the public lexical ID order without formatting hex.
            let order = row
                .id
                .algorithm()
                .as_str()
                .cmp(id.algorithm().as_str())
                .then_with(|| row.id.digest_bytes().cmp(id.digest_bytes()));
            if order.is_le() {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        Ok(low)
    }
}
pub(in crate::catalog_index) fn contents_page(
    bytes: &[u8],
    after: Option<&ObjectId>,
    limit: usize,
) -> Result<Option<Vec<ContentRef>>> {
    if !bytes.starts_with(b"PCASWRL1") {
        return Ok(None);
    }
    let list = ContentList::open(bytes)?;
    if list.len() == 0 || limit > 201 {
        return Err(Error::Integrity);
    }
    let start = after.map(|id| list.after(id)).transpose()?.unwrap_or(0);
    let end = (start + limit).min(list.len());
    (start..end)
        .map(|n| list.at(n))
        .collect::<Result<Vec<_>>>()
        .map(Some)
}



fn extension_offset(bytes: &[u8]) -> Result<usize> {
    let suffix = bytes
        .get(bytes.len().checked_sub(8).ok_or(Error::Integrity)?..)
        .ok_or(Error::Integrity)?;
    let offset = usize::try_from(u64::from_le_bytes(
        suffix.try_into().map_err(|_| Error::Integrity)?,
    ))
    .map_err(|_| Error::TooLarge)?;
    if offset < 20 || offset > bytes.len().saturating_sub(12) {
        return Err(Error::Integrity);
    }
    Ok(offset)
}
fn extension_views(bytes: &[u8]) -> Result<Vec<([u8; 32], &[u8])>> {
    if !bytes.starts_with(b"PCASPOS2") {
        return Ok(vec![]);
    }
    if bytes.len() > 8 << 20 {
        return Err(Error::TooLarge);
    }
    let start = extension_offset(bytes)?;
    let mut r = Decoder(&bytes[start..bytes.len() - 8]);
    let count = r.count()?;
    if count == 0 || count > 1024 {
        return Err(Error::Integrity);
    }
    let mut rows = vec![];
    let mut previous = None;
    for _ in 0..count {
        let context: [u8; 32] = r.take(32)?.try_into().map_err(|_| Error::Integrity)?;
        let len = r.count()?;
        if len == 0 || previous.is_some_and(|p| p >= context) {
            return Err(Error::Integrity);
        }
        previous = Some(context);
        rows.push((context, r.take(len)?));
    }
    if !r.0.is_empty() {
        return Err(Error::Integrity);
    }
    Ok(rows)
}
pub(in crate::catalog_index) fn extension<'a>(
    bytes: &'a [u8],
    context: &[u8; 32],
) -> Result<Option<&'a [u8]>> {
    Ok(extension_views(bytes)?
        .into_iter()
        .find(|(key, _)| key == context)
        .map(|(_, value)| value))
}


