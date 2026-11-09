//! Distribution experiment only. No production API or persistent format changes.
use super::*;
use crate::{
    shared_index::{
        binary::{Decoder, Encoder},
        partitions::{ValueChange, ValueSnapshot},
    },
    LocalCas,
};
use std::{collections::BTreeMap, path::Path};

const LIST_EXPERIMENT: u64 = 1 << 57;
const LIST_MAGIC: &[u8] = b"UVEXP001";

fn uuid(n: usize) -> Uuid {
    Uuid::from_u128(n as u128 + 1)
}

fn forward_record(id: Uuid, hashes: &mut Vec<ObjectId>) -> ValueChange {
    hashes.sort_unstable_by_key(identity);
    hashes.dedup();
    let mut w = Encoder::new(LIST_MAGIC, 16 + 33 * hashes.len()).unwrap();
    w.u64(hashes.len() as u64).unwrap();
    for hash in hashes {
        w.object(hash).unwrap();
    }
    ValueChange::Put(ValueRecord {
        locator: ObjectId::for_bytes(id.as_bytes()),
        key: id.as_bytes().to_vec(),
        value: w.finish(),
    })
}

fn forward<'a>(snapshot: &'a ValueSnapshot, id: &Uuid) -> Option<Cow<'a, [u8]>> {
    snapshot
        .get(&ObjectId::for_bytes(id.as_bytes()), id.as_bytes())
        .unwrap()
}

fn list(bytes: &[u8]) -> (usize, &[u8]) {
    let mut r = Decoder::new(bytes, LIST_MAGIC, 64 << 20).unwrap();
    let count = usize::try_from(r.u64().unwrap()).unwrap();
    assert_eq!(r.remaining(), count * 33);
    (count, r.raw(count * 33).unwrap())
}

fn page(bytes: &[u8], start: usize, limit: usize) -> Vec<ObjectId> {
    let (count, entries) = list(bytes);
    let start = start.min(count);
    let end = start.saturating_add(limit).min(count);
    let mut r = Decoder::new(&entries[start * 33..end * 33], b"", 64 << 20).unwrap();
    (start..end).map(|_| r.object().unwrap()).collect()
}

fn hash_file(fixture: &Fixture, label: &str, owners: &[Vec<Uuid>]) -> File {
    let baseline = ValueIndex::open(&fixture.baseline_file).unwrap();
    let old_navigation = num(&baseline.bytes, 32).unwrap();
    let mut bytes = baseline.bytes[..old_navigation].to_vec();
    for (id, uuids) in fixture.ids.iter().zip(owners) {
        assert!(!uuids.is_empty());
        let at = HEADER + baseline.probe(id) * WIDTH;
        let flags = word(&bytes, at + 40).unwrap();
        if uuids.len() == 1 {
            bytes[at + 40..at + 48].copy_from_slice(&(flags | INLINE_EXPERIMENT).to_le_bytes());
            bytes[at + 48..at + 64].copy_from_slice(uuids[0].as_bytes());
        } else {
            let offset = bytes.len();
            for id in uuids {
                bytes.extend_from_slice(id.as_bytes());
            }
            bytes[at + 40..at + 48].copy_from_slice(&(flags | LIST_EXPERIMENT).to_le_bytes());
            put(&mut bytes, at + 48, offset);
            put(&mut bytes, at + 56, uuids.len() * 16);
        }
    }
    bytes.resize((bytes.len() + 127) & !127, 0);
    let navigation = bytes.len();
    put(&mut bytes, 32, navigation);
    bytes.extend_from_slice(&baseline.bytes[old_navigation..]);
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(fixture._directory.path().join(label))
        .unwrap();
    file.write_all(&bytes).unwrap();
    file.sync_all().unwrap();
    file
}

fn reverse<'a>(index: &'a ValueIndex, id: &ObjectId) -> Option<&'a [u8]> {
    let at = HEADER + index.probe(id) * WIDTH;
    let flags = word(&index.bytes, at + 40).unwrap();
    if flags & PRESENT == 0
        || index.bytes[at..at + 32] != *id.digest_bytes()
        || (flags & ALGORITHM != 0) != (id.algorithm() == DigestAlgorithm::Blake3)
    {
        return None;
    }
    if flags & INLINE_EXPERIMENT != 0 {
        return Some(&index.bytes[at + 48..at + 64]);
    }
    assert_ne!(flags & LIST_EXPERIMENT, 0);
    let offset = num(&index.bytes, at + 48).unwrap();
    let len = num(&index.bytes, at + 56).unwrap();
    assert_eq!(len % 16, 0);
    Some(&index.bytes[offset..offset + len])
}

fn space(root: &Path) -> serde_json::Value {
    let mut pending = vec![root.to_path_buf()];
    let mut inodes = BTreeMap::new();
    let (mut paths, mut directories) = (0, 0);
    while let Some(path) = pending.pop() {
        let m = std::fs::symlink_metadata(&path).unwrap();
        if m.is_dir() {
            directories += 1;
            pending.extend(std::fs::read_dir(path).unwrap().map(|e| e.unwrap().path()));
        } else {
            assert!(m.is_file());
            paths += 1;
            inodes.insert((m.dev(), m.ino()), (m.len(), m.blocks() * 512));
        }
    }
    json!({"file_paths":paths,"unique_inodes":inodes.len(),"directories":directories,
        "logical_bytes":inodes.values().map(|v|v.0).sum::<u64>(),
        "allocated_bytes":inodes.values().map(|v|v.1).sum::<u64>()})
}

struct Versions {
    fixture: Fixture,
    store: LocalCas,
    snapshot: ValueSnapshot,
    hash_file: File,
    owners: Vec<Vec<Uuid>>,
    groups: BTreeMap<Uuid, Vec<ObjectId>>,
}

impl Versions {
    fn build(count: usize, versions: usize, shared_percent: usize) -> Self {
        assert!(count.is_multiple_of(versions));
        let fixture = Fixture::build(count);
        let mut groups = BTreeMap::<Uuid, Vec<ObjectId>>::new();
        let mut owners = vec![];
        for (n, hash) in fixture.ids.iter().enumerate() {
            let group = n / versions;
            let mut ids = vec![uuid(group)];
            if n % 100 < shared_percent {
                ids.push(uuid((group + 1) % (count / versions)));
            }
            ids.sort_unstable();
            ids.dedup();
            for id in &ids {
                groups.entry(*id).or_default().push(hash.clone());
            }
            owners.push(ids);
        }
        groups.insert(uuid(count + 1), vec![]);
        let changes: Vec<_> = groups
            .iter_mut()
            .map(|(u, hs)| forward_record(*u, hs))
            .collect();
        let store = LocalCas::open_shared(fixture._directory.path().join("uuid-index")).unwrap();
        let snapshot = ValueSnapshot::publish(&store, None, &changes, json!({})).unwrap();
        let hash_file = hash_file(&fixture, "version-hash.idx", &owners);
        Self {
            fixture,
            store,
            snapshot,
            hash_file,
            owners,
            groups,
        }
    }

    fn validate(&self) {
        let hash = ValueIndex::open(&self.hash_file).unwrap();
        for ((id, owners), n) in self.fixture.ids.iter().zip(&self.owners).zip(0..) {
            assert_eq!(lookup_position(&hash, id), Some(position_for(id)));
            let expected: Vec<_> = owners.iter().flat_map(|u| *u.as_bytes()).collect();
            assert_eq!(reverse(&hash, id), Some(expected.as_slice()));
            assert_eq!(reverse(&hash, &fixture_id(n + 10_000_000)), None);
        }
        for (id, expected) in &self.groups {
            let bytes = forward(&self.snapshot, id).unwrap();
            assert_eq!(list(&bytes).0, expected.len());
            let mut decoded = vec![];
            for start in (0..expected.len()).step_by(8) {
                decoded.extend(page(&bytes, start, 8));
            }
            assert_eq!(&decoded, expected);
            assert!(page(&bytes, expected.len(), 8).is_empty());
        }
        assert!(forward(&self.snapshot, &Uuid::nil()).is_none());
        let reopened = ValueSnapshot::open_local(&self.store, self.snapshot.reference()).unwrap();
        assert_eq!(
            forward(&reopened, &uuid(0)),
            forward(&self.snapshot, &uuid(0))
        );
    }

    fn append(&mut self, versions: usize, shared_percent: usize) {
        let count = self.fixture.ids.len();
        let start = Instant::now();
        let mut changes = vec![];
        for n in 0..100.min(self.groups.len() - 1) {
            let id = uuid(n);
            let bytes = forward(&self.snapshot, &id).unwrap();
            let mut hashes = page(&bytes, 0, list(&bytes).0);
            hashes.push(fixture_id(count + n));
            changes.push(forward_record(id, &mut hashes));
            self.groups.insert(id, hashes);
            self.owners.push(vec![id]);
        }
        let prepare_ns = start.elapsed().as_nanos();
        let before = space(self.store.root());
        let start = Instant::now();
        let next =
            ValueSnapshot::publish(&self.store, Some(&self.snapshot), &changes, json!({})).unwrap();
        let forward_publish_ns = start.elapsed().as_nanos();
        let start = Instant::now();
        // Whole-leaf prototype rebuild, explicitly including fixture preparation.
        // This is NOT the production hash-partition incremental publication cost.
        let next_fixture = Fixture::build(self.owners.len());
        let next_hash_file = hash_file(&next_fixture, "appended-hash.idx", &self.owners);
        let hash_rebuild_with_fixture_ns = start.elapsed().as_nanos();
        let next_hash = ValueIndex::open(&next_hash_file).unwrap();
        for (n, _) in changes.iter().enumerate() {
            let id = uuid(n);
            let old_count = list(&forward(&self.snapshot, &id).unwrap()).0;
            let bytes = forward(&next, &id).unwrap();
            assert_eq!(list(&bytes).0, old_count + 1);
            assert_eq!(page(&bytes, 0, list(&bytes).0), self.groups[&id]);
            assert_eq!(
                reverse(&next_hash, &fixture_id(count + n)),
                Some(id.as_bytes().as_slice())
            );
        }
        let after = space(self.store.root());
        println!(
            "{}",
            json!({"kind":"uuid_versions_append","rows":count,"versions":versions,"shared_percent":shared_percent,
            "new_hashes":changes.len(),"prepare_ns":prepare_ns,"forward_publish_ns":forward_publish_ns,
            "hash_rebuild_including_fixture_ns":hash_rebuild_with_fixture_ns,
            "uuid_store_before":before,"uuid_store_with_both_snapshots":after,
            "old_hash_bytes":self.hash_file.metadata().unwrap().len(),"new_hash_bytes":next_hash_file.metadata().unwrap().len(),
            "atomic_two_direction_publication":false})
        );
    }
}

#[inline(never)]
fn sample(mut call: impl FnMut(usize), queries: usize) -> f64 {
    for n in 0..queries {
        black_box(&mut call)(n);
    }
    let start = Instant::now();
    for _ in 0..8 {
        for n in 0..queries {
            black_box(&mut call)(n);
        }
    }
    start.elapsed().as_nanos() as f64 / (8 * queries) as f64
}

#[test]
fn versions_and_sparse_shared_content_roundtrip_and_append() {
    for versions in [1, 10, 100] {
        for shared in [0, 1] {
            let mut model = Versions::build(1_000, versions, shared);
            model.validate();
            model.append(versions, shared);
        }
    }
}

#[test]
#[ignore = "Release-only distribution experiment; use the Host executor"]
fn uuid_versions_release_experiment() {
    assert!(!black_box(cfg!(debug_assertions)), "Release required");
    let _cpu = QueryCpu::pin();
    let digest =
        ObjectId::for_reader(&mut File::open(std::env::current_exe().unwrap()).unwrap()).unwrap();
    println!(
        "{}",
        json!({"kind":"uuid_versions_conditions","digest":digest.to_string(),"rows":100_000,
        "versions":[1,10,100],"shared_percent":[0,1],"rounds":5,"page_size":8,
        "scope":"hash native leaf plus complete UUID ValueSnapshot store; no domain originals", "production_format":false})
    );
    for versions in [1, 10, 100] {
        for shared in [0, 1] {
            let start = Instant::now();
            let mut model = Versions::build(100_000, versions, shared);
            let build_ns = start.elapsed().as_nanos();
            model.validate();
            let base = ValueIndex::open(&model.fixture.baseline_file).unwrap();
            let hash = ValueIndex::open(&model.hash_file).unwrap();
            let uuid_space = space(model.store.root());
            println!(
                "{}",
                json!({"kind":"uuid_versions_space","versions":versions,"shared_percent":shared,
                "hashes":100_000,"uuids_including_empty":model.groups.len(),"relations":model.owners.iter().map(Vec::len).sum::<usize>(),
                "baseline_hash_bytes":model.fixture.baseline_file.metadata().unwrap().len(),
                "hash_bytes":model.hash_file.metadata().unwrap().len(),"hash_allocated_bytes":model.hash_file.metadata().unwrap().blocks()*512,
                "hash_files":1,"uuid_store":uuid_space,"build_with_fixture_ns":build_ns})
            );
            let trace = model.fixture.trace(true);
            let keys: Vec<_> = model.groups.keys().copied().collect();
            for round in 0..5 {
                let (a, b) = if round % 2 == 0 {
                    (
                        measure(&base, &trace, Query::Position),
                        measure(&hash, &trace, Query::Position),
                    )
                } else {
                    let b = measure(&hash, &trace, Query::Position);
                    (measure(&base, &trace, Query::Position), b)
                };
                println!(
                    "{}",
                    json!({"kind":"uuid_versions_position","versions":versions,"shared_percent":shared,"round":round,"baseline_ns":a,"relation_ns":b,"ratio":b/a})
                );
                for operation in [
                    "uuid_count",
                    "uuid_first_page",
                    "uuid_last_page",
                    "uuid_missing",
                    "hash_uuids",
                ] {
                    let ns = sample(
                        |n| {
                            if operation == "hash_uuids" {
                                black_box(reverse(&hash, black_box(&trace[n % trace.len()])));
                            } else {
                                let id = if operation == "uuid_missing" {
                                    Uuid::nil()
                                } else {
                                    keys[(n * 7919) % keys.len()]
                                };
                                let result = forward(&model.snapshot, black_box(&id));
                                if let Some(bytes) = result {
                                    let count = list(&bytes).0;
                                    if operation == "uuid_count" {
                                        black_box(count);
                                    } else {
                                        black_box(page(
                                            &bytes,
                                            if operation == "uuid_last_page" {
                                                count.saturating_sub(8)
                                            } else {
                                                0
                                            },
                                            8,
                                        ));
                                    }
                                } else {
                                    black_box(result);
                                }
                            }
                        },
                        4096,
                    );
                    println!(
                        "{}",
                        json!({"kind":"uuid_versions_query","versions":versions,"shared_percent":shared,"round":round,"operation":operation,"ns_per_query":ns})
                    );
                }
            }
            model.append(versions, shared);
        }
    }
}

mod deferred;
