use super::*;
use crate::{manifest::TreeEntry, ObjectId};
use http::{
    header::{HeaderName, HeaderValue},
    Uri,
};
use std::collections::BTreeSet;

fn invalid(message: &'static str) -> Error {
    Error::Invalid(message.into())
}

fn http_uri(value: &str) -> Result<Uri> {
    if value.len() > 16 * 1024 || value.contains('#') {
        return Err(invalid("invalid HTTP read URI"));
    }
    let uri: Uri = value
        .parse()
        .map_err(|_| invalid("invalid HTTP read URI"))?;
    if !matches!(uri.scheme_str(), Some("http" | "https"))
        || uri.authority().is_none_or(|v| v.as_str().contains('@'))
        || uri.host().is_none_or(str::is_empty)
    {
        return Err(invalid("expected absolute HTTP read URI without userinfo"));
    }
    Ok(uri)
}

pub(super) fn temporary_request(request: &TemporaryRequest) -> Result<()> {
    if request.method != "GET"
        || request.expires_at_ms == 0
        || request.expires_at_ms > i64::MAX as u64
        || request.headers.len() > 32
    {
        return Err(invalid("invalid temporary GET request metadata"));
    }
    let uri = http_uri(&request.url)?;
    let mut names = BTreeSet::new();
    for (key, value) in &request.headers {
        if key.len() > 128 || value.len() > 8192 {
            return Err(Error::TooLarge);
        }
        let name = HeaderName::from_bytes(key.as_bytes())
            .map_err(|_| invalid("invalid temporary request header"))?;
        HeaderValue::from_bytes(value.as_bytes())
            .map_err(|_| invalid("invalid temporary request header"))?;
        if !names.insert(name.as_str().to_owned()) {
            return Err(invalid("duplicate temporary request header"));
        }
        if name == http::header::HOST
            && uri
                .authority()
                .is_none_or(|v| !v.as_str().eq_ignore_ascii_case(value))
        {
            return Err(invalid("temporary request Host does not match URI"));
        }
    }
    Ok(())
}

impl StorageDescriptor {
    pub fn validate(&self) -> Result<()> {
        crate::manifest::validate_name(&self.name)?;
        if self.config_version != 1 {
            return Err(invalid("unknown storage descriptor version"));
        }
        crate::source::validate_public_storage(&self.adapter, &self.binding)?;
        if matches!(
            self.adapter.as_str(),
            "s3" | "opendal-s3" | "http" | "opendal-http"
        ) {
            let endpoint = self
                .binding
                .get("endpoint")
                .ok_or_else(|| invalid("missing public endpoint"))?;
            http_uri(endpoint)?;
            if matches!(self.adapter.as_str(), "s3" | "opendal-s3")
                && (self.binding.get("bucket").is_none_or(String::is_empty)
                    || !self.binding.contains_key("root"))
            {
                return Err(invalid("S3 descriptor requires bucket and explicit root"));
            }
        }
        Ok(())
    }
}

fn remember<'a>(
    refs: &mut BTreeMap<String, &'a ContentRef>,
    content: &'a ContentRef,
) -> Result<()> {
    if refs
        .insert(content.id.to_string(), content)
        .is_some_and(|old| old.size_bytes != content.size_bytes)
    {
        return Err(Error::Integrity);
    }
    Ok(())
}

pub(super) fn validate(draft: &BundleDraft, limits: BundleLimits) -> Result<TreeClosure> {
    if draft.directories.len() > limits.max_directories
        || draft.storages.len() > limits.max_storages
        || draft.sources.len() > limits.max_sources
    {
        return Err(Error::TooLarge);
    }
    let mut nodes = BTreeMap::new();
    let mut refs = BTreeMap::new();
    remember(&mut refs, &draft.root)?;
    let mut directory_bytes = 0usize;
    for record in &draft.directories {
        let bytes = record.canonical_json.as_bytes();
        directory_bytes = directory_bytes
            .checked_add(bytes.len())
            .ok_or(Error::TooLarge)?;
        if directory_bytes > limits.max_bundle_bytes {
            return Err(Error::TooLarge);
        }
        if record.content.size_bytes != bytes.len() as u64
            || ObjectId::for_bytes_with(record.content.id.algorithm(), bytes) != record.content.id
        {
            return Err(Error::Integrity);
        }
        let tree = TreeManifest::from_bytes(bytes, limits.tree)?;
        remember(&mut refs, &record.content)?;
        if nodes.insert(record.content.id.to_string(), tree).is_some() {
            return Err(invalid("duplicate directory record"));
        }
    }
    // Expand logical paths, not just unique nodes: a small DAG must not bypass
    // output-entry/byte/depth limits. This walk never opens a source.
    let mut queue = vec![(&draft.root, 0usize)];
    let mut visited = BTreeSet::new();
    let mut files = BTreeMap::new();
    let mut logical_entry_count = 0usize;
    let mut logical_file_bytes = 0u64;
    while let Some((reference, depth)) = queue.pop() {
        if depth > limits.tree.max_depth {
            return Err(Error::TooLarge);
        }
        remember(&mut refs, reference)?;
        let node_id = reference.id.to_string();
        let tree = nodes
            .get(&node_id)
            .ok_or_else(|| invalid("missing embedded directory"))?;
        visited.insert(node_id);
        logical_entry_count = logical_entry_count
            .checked_add(tree.entries().len())
            .ok_or(Error::TooLarge)?;
        if logical_entry_count > limits.tree.max_entries {
            return Err(Error::TooLarge);
        }
        for entry in tree.entries() {
            if let Some(content) = entry.content() {
                remember(&mut refs, content)?;
            }
            match entry {
                TreeEntry::Directory { manifest, .. } => {
                    queue.push((manifest, depth.checked_add(1).ok_or(Error::TooLarge)?))
                }
                TreeEntry::File { content, .. } => {
                    logical_file_bytes = logical_file_bytes
                        .checked_add(content.size_bytes)
                        .ok_or(Error::TooLarge)?;
                    if logical_file_bytes > limits.tree.max_total_bytes {
                        return Err(Error::TooLarge);
                    }
                    files.insert(content.id.to_string(), content.clone());
                }
                TreeEntry::Symlink { .. } => {}
            }
        }
    }
    if visited.len() != nodes.len() {
        return Err(invalid("unreachable embedded directory"));
    }
    let mut storages = BTreeSet::new();
    for storage in &draft.storages {
        storage.validate()?;
        if !storages.insert(storage.name.as_str()) {
            return Err(invalid("duplicate storage descriptor"));
        }
    }
    let mut routed = BTreeMap::new();
    let mut earliest_expiry_ms = None;
    for source in &draft.sources {
        if source.routes.len() > limits.max_routes_per_object {
            return Err(Error::TooLarge);
        }
        let key = source.content.id.to_string();
        if refs.get(&key).copied() != Some(&source.content) {
            return Err(invalid("source content is not a reachable tree reference"));
        }
        if routed.insert(key, &source.routes).is_some() {
            return Err(invalid("duplicate content sources"));
        }
        let mut unique_routes = BTreeSet::new();
        for route in &source.routes {
            route.locator().validate()?;
            if !storages.contains(route.storage()) {
                return Err(invalid("missing route storage descriptor"));
            }
            // One route of each access kind per exact location/revision. A
            // refreshed request replaces a capability, not content identity.
            if !unique_routes.insert((
                route.storage(),
                &route.locator().key,
                &route.locator().revision,
                matches!(route, ReadRoute::Temporary { .. }),
            )) {
                return Err(invalid("duplicate content read route"));
            }
            if let ReadRoute::Temporary { request, .. } = route {
                request.validate()?;
                earliest_expiry_ms = Some(
                    earliest_expiry_ms.map_or(request.expires_at_ms, |old: u64| {
                        old.min(request.expires_at_ms)
                    }),
                );
            }
        }
    }
    let missing_routes: Vec<_> = files
        .iter()
        .filter_map(|(id, content)| {
            let usable = routed.get(id).is_some_and(|routes| {
                routes.iter().any(|route| {
                    draft.access == AccessMode::Temporary
                        || matches!(route, ReadRoute::Durable { .. })
                })
            });
            (!usable).then(|| content.clone())
        })
        .collect();
    if draft.completeness == Completeness::Consumable && !missing_routes.is_empty() {
        return Err(invalid("consumable bundle lacks required read routes"));
    }
    Ok(TreeClosure {
        files: files.into_values().collect(),
        missing_routes,
        logical_entry_count,
        logical_file_bytes,
        earliest_expiry_ms,
    })
}
