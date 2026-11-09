//! Private layout experiment, not a production format or publication path.
//! Reuses the real native builder, navigator and local position decoder.
use super::*;
use serde_json::json;
use std::{hint::black_box, os::unix::fs::MetadataExt, time::Instant};
use uuid::Uuid;

mod versions;

const INLINE_EXPERIMENT: u64 = 1 << 58;
const QUERIES: usize = 32_768;
const PASSES: usize = 8;

fn fixture_id(n: usize) -> ObjectId {
    let digest = *ObjectId::for_bytes(&(n as u64).to_le_bytes()).digest_bytes();
    ObjectId::from_digest(
        if n.is_multiple_of(2) {
            DigestAlgorithm::Sha256
        } else {
            DigestAlgorithm::Blake3
        },
        digest,
    )
}

fn position_for(id: &ObjectId) -> LocatedContent {
    LocatedContent {
        content: ContentRef {
            id: id.clone(),
            size_bytes: u64::from_le_bytes(id.digest_bytes()[..8].try_into().unwrap()) >> 16,
        },
        location: if id.digest_bytes()[0].is_multiple_of(2) {
            PhysicalCandidate::Loose
        } else {
            PhysicalCandidate::PackMember {
                pack: ContentRef {
                    id: ObjectId::for_bytes(b"uuid-inline-experiment-pack"),
                    size_bytes: 123_456,
                },
            }
        },
    }
}

fn fixture_local(record: &ValueRecord) -> Result<Option<LocatedContent>> {
    let position = position_for(&record.locator);
    assert_eq!(record.value, position::single_local_value(&position)?);
    Ok(Some(position))
}

struct Fixture {
    _directory: tempfile::TempDir,
    baseline_file: File,
    inline_file: File,
    ids: Vec<ObjectId>,
}

impl Fixture {
    fn build(count: usize) -> Self {
        let directory = tempfile::Builder::new()
            .prefix("uuid-inline-experiment-")
            .tempdir_in("/data/polar/cache/product-test-runtime")
            .unwrap();
        let ids: Vec<_> = (0..count).map(fixture_id).collect();
        let records: Vec<_> = ids
            .iter()
            .map(|id| ValueRecord {
                locator: id.clone(),
                key: identity(id).to_vec(),
                value: position::single_local_value(&position_for(id)).unwrap(),
            })
            .collect();
        let mut baseline_file = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(directory.path().join("baseline.idx"))
            .unwrap();
        ValueIndex::try_build_with_local(&mut baseline_file, &records, Some(fixture_local))
            .unwrap()
            .expect("fixture must use actual PtrHash navigation");
        let baseline = ValueIndex::open(&baseline_file).unwrap();
        let mut bytes = baseline.bytes.to_vec();
        for (n, id) in ids.iter().enumerate() {
            let at = HEADER + baseline.probe(id) * WIDTH;
            let flags = word(&bytes, at + 40).unwrap();
            assert_eq!(flags & LOCAL_ONLY, LOCAL_ONLY);
            assert_eq!(&bytes[at + 48..at + 64], &[0; 16]);
            bytes[at + 40..at + 48].copy_from_slice(&(flags | INLINE_EXPERIMENT).to_le_bytes());
            bytes[at + 48..at + 64].copy_from_slice(Uuid::from_u128(n as u128 + 1).as_bytes());
        }
        // No production store, CAS object or published inode is ever modified.
        let mut inline_file = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(directory.path().join("experiment-only.idx"))
            .unwrap();
        inline_file.write_all(&bytes).unwrap();
        inline_file.sync_all().unwrap();
        assert_eq!(baseline_file.metadata().unwrap().len(), bytes.len() as u64);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
        Self {
            _directory: directory,
            baseline_file,
            inline_file,
            ids,
        }
    }

    fn validate(&self) {
        // Open independent mappings, and explicitly exercise closing/reopening.
        let baseline = ValueIndex::open(&self.baseline_file).unwrap();
        let inline = ValueIndex::open(&self.inline_file).unwrap();
        assert_eq!(baseline.slots, inline.slots);
        assert_eq!(baseline.payload, inline.payload);
        assert_eq!(baseline.dictionary, inline.dictionary);
        let navigation = num(&baseline.bytes, 32).unwrap();
        assert_eq!(&baseline.bytes[navigation..], &inline.bytes[navigation..]);
        for (n, id) in self.ids.iter().enumerate() {
            let expected = Some(position_for(id));
            assert_eq!(lookup_position(&baseline, id), expected);
            assert_eq!(lookup_position(&inline, id), expected);
            assert_eq!(lookup_uuid(&baseline, id), None);
            assert_eq!(
                lookup_uuid(&inline, id),
                Some(Uuid::from_u128(n as u128 + 1))
            );
            let wrong_algorithm = ObjectId::from_digest(
                if id.algorithm() == DigestAlgorithm::Sha256 {
                    DigestAlgorithm::Blake3
                } else {
                    DigestAlgorithm::Sha256
                },
                *id.digest_bytes(),
            );
            assert_eq!(lookup_position(&inline, &wrong_algorithm), None);
            assert_eq!(lookup_uuid(&inline, &wrong_algorithm), None);
        }
        // The prototype is intentionally NOT a completed portable format.
        assert!(portable::decode(portable_prefix(&inline.bytes).unwrap()).is_err());
        assert!(inline.row(&self.ids[0]).is_err());
        assert_eq!(
            decode_records(portable_prefix(&baseline.bytes).unwrap())
                .unwrap()
                .len(),
            self.ids.len()
        );
        let trace = self.trace(true);
        for chunk in trace.chunks(512) {
            let a = lookup_batch(&baseline, chunk);
            let b = lookup_batch(&inline, chunk);
            assert_eq!(a, b);
            for (id, result) in chunk.iter().zip(b) {
                assert_eq!(result, Some(position_for(id)));
            }
        }
        for id in self.trace(false) {
            assert_eq!(lookup_position(&baseline, &id), None);
            assert_eq!(lookup_position(&inline, &id), None);
            assert_eq!(lookup_uuid(&inline, &id), None);
        }
        drop(inline);
        assert_eq!(
            lookup_uuid(&ValueIndex::open(&self.inline_file).unwrap(), &self.ids[0]),
            Some(Uuid::from_u128(1))
        );
    }

    fn trace(&self, hits: bool) -> Vec<ObjectId> {
        let mut state = 0x853c_49e6_748f_ea9bu64;
        (0..QUERIES)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                let n = (state >> 32) as usize % self.ids.len();
                if hits {
                    self.ids[n].clone()
                } else {
                    fixture_id(n + 10_000_000)
                }
            })
            .collect()
    }
}

fn lookup_position(index: &ValueIndex, id: &ObjectId) -> Option<LocatedContent> {
    let mut found = None;
    assert!(index.local_into(index.probe(id), id, &mut found).unwrap());
    found
}

fn lookup_uuid(index: &ValueIndex, id: &ObjectId) -> Option<Uuid> {
    let at = HEADER + index.probe(id) * WIDTH;
    let flags = word(&index.bytes, at + 40).unwrap();
    if flags & (PRESENT | INLINE_EXPERIMENT) != PRESENT | INLINE_EXPERIMENT
        || index.bytes[at..at + 32] != *id.digest_bytes()
        || (flags & ALGORITHM != 0) != (id.algorithm() == DigestAlgorithm::Blake3)
    {
        return None;
    }
    Some(Uuid::from_bytes(
        index.bytes[at + 48..at + 64].try_into().unwrap(),
    ))
}

fn lookup_batch(index: &ValueIndex, ids: &[ObjectId]) -> Vec<Option<LocatedContent>> {
    let mut slots = [0; 512];
    let mut out = vec![None; ids.len()];
    batch_into(index, ids, &mut slots, &mut out);
    out
}

fn batch_into(
    index: &ValueIndex,
    ids: &[ObjectId],
    slots: &mut [usize; 512],
    out: &mut [Option<LocatedContent>],
) {
    // Same prefetch / local_into sequence used by ValueSnapshot's batch path.
    for (slot, id) in slots.iter_mut().zip(ids) {
        *slot = index.prefetch_probe(id);
    }
    for ((slot, id), result) in slots.iter().zip(ids).zip(out) {
        assert!(index.local_into(*slot, id, result).unwrap());
    }
}

#[derive(Clone, Copy, Debug)]
enum Query {
    Position,
    Batch,
    Uuid,
}

#[inline(never)]
fn measure(index: &ValueIndex, ids: &[ObjectId], query: Query) -> f64 {
    let mut slots = [0; 512];
    let mut out = vec![None; 512];
    let mut trace = || match query {
        Query::Position => {
            for id in ids {
                black_box(lookup_position(black_box(index), black_box(id)));
            }
        }
        Query::Uuid => {
            for id in ids {
                black_box(lookup_uuid(black_box(index), black_box(id)));
            }
        }
        Query::Batch => {
            for chunk in ids.chunks(512) {
                batch_into(
                    black_box(index),
                    black_box(chunk),
                    &mut slots,
                    &mut out[..chunk.len()],
                );
                black_box(&out[..chunk.len()]);
            }
        }
    };
    for _ in 0..3 {
        black_box(&mut trace)();
    }
    let start = Instant::now();
    for _ in 0..PASSES {
        black_box(&mut trace)();
    }
    start.elapsed().as_nanos() as f64 / (ids.len() * PASSES) as f64
}

struct QueryCpu(rustix::thread::CpuSet);
impl QueryCpu {
    fn pin() -> Self {
        use rustix::thread::{sched_getaffinity, sched_setaffinity, CpuSet};
        let original = sched_getaffinity(None).unwrap();
        let cpu = (0..CpuSet::MAX_CPU)
            .find(|cpu| original.is_set(*cpu))
            .unwrap();
        let mut selected = CpuSet::new();
        selected.set(cpu);
        sched_setaffinity(None, &selected).unwrap();
        println!("{}", json!({"kind":"uuid_inline_cpu", "cpu":cpu}));
        Self(original)
    }
}
impl Drop for QueryCpu {
    fn drop(&mut self) {
        rustix::thread::sched_setaffinity(None, &self.0).unwrap();
    }
}

#[test]
fn inline_uuid_layout_preserves_positions_and_exact_identity() {
    Fixture::build(1_000).validate();
}

#[test]
#[ignore = "Release-only small layout experiment; run through the Host executor"]
fn uuid_inline_small_release_experiment() {
    assert!(
        !black_box(cfg!(debug_assertions)),
        "timing requires Release"
    );
    let _cpu = QueryCpu::pin();
    let binary = std::env::current_exe().unwrap();
    let digest = ObjectId::for_reader(&mut File::open(&binary).unwrap()).unwrap();
    println!(
        "{}",
        json!({"kind":"uuid_inline_conditions", "binary":binary,
        "digest":digest.to_string(), "unix_ms":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis(),
        "rows":[10_000,100_000], "queries":QUERIES, "passes":PASSES, "rounds":7,
        "batch":512, "cache":"warm", "scope":"native leaf only; no Catalog root or UUID forward index", "production_format":false})
    );
    for count in [10_000, 100_000] {
        let fixture = Fixture::build(count);
        fixture.validate();
        let baseline = ValueIndex::open(&fixture.baseline_file).unwrap();
        let inline = ValueIndex::open(&fixture.inline_file).unwrap();
        for (label, file) in [
            ("baseline", &fixture.baseline_file),
            ("inline", &fixture.inline_file),
        ] {
            let metadata = file.metadata().unwrap();
            println!(
                "{}",
                json!({"kind":"uuid_inline_space", "rows":count, "layout":label,
                "logical_bytes":metadata.len(), "allocated_bytes":metadata.blocks()*512,
                "files":1, "slots":baseline.slots, "slot_width":WIDTH})
            );
        }
        let hits = fixture.trace(true);
        let misses = fixture.trace(false);
        for (name, query, ids) in [
            ("position_hit", Query::Position, &hits),
            ("position_miss", Query::Position, &misses),
            ("position_batch_hit", Query::Batch, &hits),
            ("position_batch_miss", Query::Batch, &misses),
        ] {
            for round in 0..7 {
                let (a, b) = if round % 2 == 0 {
                    (measure(&baseline, ids, query), measure(&inline, ids, query))
                } else {
                    let b = measure(&inline, ids, query);
                    (measure(&baseline, ids, query), b)
                };
                println!(
                    "{}",
                    json!({"kind":"uuid_inline_sample", "rows":count, "query":name,
                    "round":round,"baseline_first":round%2==0,"baseline_ns_per_query":a,"inline_ns_per_query":b,"ratio":b/a})
                );
            }
        }
        for (name, ids) in [("uuid_hit", &hits), ("uuid_miss", &misses)] {
            for round in 0..7 {
                println!(
                    "{}",
                    json!({"kind":"uuid_inline_uuid_sample", "rows":count, "query":name,
                    "round":round,"ns_per_query":measure(&inline, ids, Query::Uuid)})
                );
            }
        }
    }
}
