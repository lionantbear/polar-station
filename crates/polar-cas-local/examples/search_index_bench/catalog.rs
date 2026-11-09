//! Compare actual Catalog lookup with the original SearchIndex. All fixture
//! publication, open, validation and file accounting are outside lookup timing.
use super::accounting::{self, Phase};
use polar_cas::{
    shared::{CompletePackIndex, LocatedContent, PhysicalCandidate},
    ContentRef, ObjectId,
};
use polar_cas_local::{
    catalog_index::UnifiedCatalogIndex,
    shared_index::{SearchIndex, SharedPackStore},
    LocalCas,
};
use serde_json::json;
use std::{fs::OpenOptions, hint::black_box, path::Path, sync::atomic::AtomicBool, time::Instant};

fn id(n: usize) -> ObjectId {
    ObjectId::for_bytes(&(n as u64).to_le_bytes())
}
// Keep each sample's measured trace independent of inlining/CSE and give both
// implementations identical warm-up and repeated work, not one switched trace.
const PASSES: u128 = 16;
#[inline(never)]
fn measure(mut f: impl FnMut() -> u64) -> (u128, u64) {
    for _ in 0..3 {
        black_box(black_box(&mut f)());
    }
    let start = Instant::now();
    let mut checksum = 0;
    for _ in 0..PASSES {
        checksum = black_box(black_box(&mut f)());
    }
    (start.elapsed().as_nanos() / PASSES, checksum)
}
fn checksum(hit: &Option<LocatedContent>) -> u64 {
    black_box(hit).as_ref().map_or(0, |hit| {
        hit.content.size_bytes ^ u64::from(hit.content.id.digest_bytes()[0])
    })
}
#[cfg(target_os = "linux")]
struct QueryCpu(rustix::thread::CpuSet);
#[cfg(target_os = "linux")]
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
        println!(
            "{}",
            json!({"kind":"catalog_comparison_cpu","cpu":cpu,"selection":"first allowed CPU","lookup_thread_only":true})
        );
        Self(original)
    }
}
#[cfg(target_os = "linux")]
impl Drop for QueryCpu {
    fn drop(&mut self) {
        rustix::thread::sched_setaffinity(None, &self.0).unwrap();
    }
}
pub fn run(root: &Path) -> bool {
    let stop = AtomicBool::new(false);
    let mut all_passed = true;
    for count in [1_000_000usize, 10_000_000] {
        let temp = tempfile::Builder::new()
            .prefix("catalog-comparison-")
            .tempdir_in(root)
            .unwrap();
        let store = LocalCas::open_shared(temp.path().join("objects")).unwrap();
        accounting::space("hash", "empty", count, store.root());
        let publisher = SharedPackStore::open(store.clone()).unwrap();
        let mut packs = Vec::new();
        let build = Instant::now();
        for (pack_number, start) in (0..count).step_by(500_000).enumerate() {
            // Metadata-only fixture, as in the existing shared index benchmark.
            // Bodies are not opened by either lookup implementation.
            let pack = store
                .publish_shared_bytes(&vec![pack_number as u8; 4096])
                .unwrap();
            let members = (start..(start + 500_000).min(count))
                .map(|n| ContentRef {
                    id: id(n),
                    size_bytes: n as u64 + 1,
                })
                .collect();
            let admission = CompletePackIndex::from_trusted_adapter(pack.clone(), members).unwrap();
            publisher
                .prepare(&admission, &stop)
                .unwrap()
                .publish()
                .unwrap();
            packs.push(pack);
        }
        // All pending Packs fit the existing 32-Pack aggregate batch.
        publisher.aggregate(&stop).unwrap();
        let catalog_build_ns = build.elapsed().as_nanos();
        let phase = Phase::start();
        let catalog = UnifiedCatalogIndex::open_shared(store.clone()).unwrap();
        catalog.refresh().unwrap();
        let snapshot = catalog.snapshot().unwrap();
        phase.finish("hash", "catalog-open-and-refresh", count);
        let before_query = accounting::space("hash", "built-all-generations", count, store.root());
        accounting::active(
            "hash",
            snapshot.root().unwrap(),
            &snapshot.partition_refs(),
            count,
            "built",
        );
        let pack_admission =
            accounting::inventory(&store.root().join(".polar-pack-index-v1")).unwrap();
        println!(
            "{}",
            json!({"kind":"pack_admission_space", "rows":count, "space":pack_admission})
        );
        let payload_bytes: u64 = packs.iter().map(|p| p.size_bytes).sum();
        println!(
            "{}",
            json!({"kind":"catalog_index_accounting","rows":count,
            "synthetic_pack_files":packs.len(),"synthetic_pack_bytes":payload_bytes,
            "index_and_control_logical_bytes":before_query.unique_logical_bytes-payload_bytes,
            "index_and_control_file_paths":before_query.file_paths-packs.len() as u64,
            "logical_partitions":snapshot.partition_count(),"includes_old_and_prepared_indexes":true})
        );
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(temp.path().join("original.idx"))
            .unwrap();
        let build = Instant::now();
        let mut rows: Vec<_> = (0..count)
            .map(|n| LocatedContent {
                content: ContentRef {
                    id: id(n),
                    size_bytes: n as u64 + 1,
                },
                location: PhysicalCandidate::PackMember {
                    pack: packs[n / 500_000].clone(),
                },
            })
            .collect();
        // The original streaming builder requires canonical identity order.
        rows.sort_unstable_by_key(|row| *row.content.id.digest_bytes());
        let (digest, actual) =
            SearchIndex::build_fast(&mut file, rows.into_iter().map(Ok), temp.path(), &stop)
                .unwrap();
        assert_eq!(actual, count as u64);
        let old = SearchIndex::open(&file, Some(&digest)).unwrap();
        println!(
            "{}",
            json!({"kind":"catalog_comparison_build","rows":count,"catalog_ns":catalog_build_ns,"original_ns":build.elapsed().as_nanos(),"logical_partitions":snapshot.partition_count(),"payload_io_in_lookup":false})
        );
        println!(
            "{}",
            json!({"kind":"original_index_space","rows":count,
            "file_paths":1,"logical_bytes":file.metadata().unwrap().len()})
        );
        #[cfg(target_os = "linux")]
        let _cpu = QueryCpu::pin();
        // Fixed permutation-like trace, including 20% misses and repeat keys.
        let ids: Vec<_> = (0..100_000usize)
            .map(|n| {
                id(if n % 5 == 0 {
                    count + n
                } else {
                    n.wrapping_mul(104729) % count
                })
            })
            .collect();
        let expected: Vec<_> = ids.iter().map(|id| old.locate(id).unwrap()).collect();
        for (id, expected) in ids.iter().zip(&expected) {
            assert_eq!(&snapshot.locate_local(id).unwrap(), expected);
        }
        for (batch, wanted) in ids.chunks(1024).zip(expected.chunks(1024)) {
            let mut results = vec![None; batch.len()];
            snapshot
                .locate_local_batch_into(batch, &mut results)
                .unwrap();
            assert_eq!(results, wanted);
        }
        let warmup = Instant::now();
        while warmup.elapsed() < std::time::Duration::from_secs(2) {
            for id in &ids {
                black_box(old.locate(black_box(id)).unwrap());
                black_box(snapshot.locate_local(black_box(id)).unwrap());
            }
        }
        let mut timings = [[Vec::new(), Vec::new()], [Vec::new(), Vec::new()]];
        for round in 0..8 {
            // Alternate measurement order; both paths have been warmed by full
            // output validation. Never label reopened warm OS pages as cold.
            for modern in if round % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            } {
                let single_work = || {
                    ids.iter()
                        .map(|id| {
                            checksum(&if modern {
                                snapshot.locate_local(id).unwrap()
                            } else {
                                old.locate(id).unwrap()
                            })
                        })
                        .fold(0, u64::wrapping_add)
                };
                let (single_ns, single_sum) = measure(single_work);
                let mut results = vec![None; 1024];
                let batch_work = || {
                    let mut sum = 0u64;
                    for batch in ids.chunks(1024) {
                        let out = &mut results[..batch.len()];
                        if modern {
                            snapshot.locate_local_batch_into(batch, out).unwrap();
                        } else {
                            old.locate_batch_into::<32>(batch, out).unwrap();
                        }
                        for result in out {
                            sum = sum.wrapping_add(checksum(result));
                        }
                    }
                    sum
                };
                let (batch_ns, batch_sum) = measure(batch_work);
                assert_eq!(single_sum, batch_sum);
                timings[0][usize::from(modern)].push(single_ns);
                timings[1][usize::from(modern)].push(batch_ns);
                println!(
                    "{}",
                    json!({"kind":"catalog_comparison_lookup","rows":count,"round":round,"implementation":if modern {"CatalogSnapshot"} else {"SearchIndex"},"cache":"warm_os_and_trace","queries_per_pass":ids.len(),"passes_per_sample":PASSES,"single_ns":single_ns,"batch_ns":batch_ns,"checksum":single_sum,"full_output_verified":true,"payload_io":false})
                );
            }
        }
        let mut passed = true;
        for (mode, implementations) in ["single", "batch"].into_iter().zip(&mut timings) {
            for samples in implementations.iter_mut() {
                samples.sort_unstable();
            }
            let median = |samples: &[u128]| (samples[3] + samples[4]) as f64 / 2.0;
            let ratio = median(&implementations[1]) / median(&implementations[0]);
            println!(
                "{}",
                json!({"kind":"catalog_comparison_gate","rows":count,"mode":mode,"ratio":ratio,"threshold":1.05,"passed":ratio <= 1.05})
            );
            passed &= ratio <= 1.05;
        }
        all_passed &= passed;
        // Instrumented single-query latency is separate from throughput timing.
        let samples = ids
            .iter()
            .map(|id| {
                let start = Instant::now();
                black_box(snapshot.locate_local(black_box(id)).unwrap());
                start.elapsed().as_nanos()
            })
            .collect();
        accounting::latency("hash", &format!("catalog-single-{count}"), samples);
        // Release the single-thread CPU pin before measuring concurrent readers.
        #[cfg(target_os = "linux")]
        drop(_cpu);
        for workers in [1usize, 8, 32] {
            let ready = std::sync::Barrier::new(workers + 1);
            let wall_ns = std::thread::scope(|scope| {
                let handles: Vec<_> = (0..workers)
                    .map(|_| {
                        let snapshot = &snapshot;
                        let ids = &ids;
                        let expected = &expected;
                        let ready = &ready;
                        scope.spawn(move || {
                            let mut out = vec![None; ids.len()];
                            ready.wait();
                            ready.wait();
                            let start = Instant::now();
                            for (id, result) in ids.iter().zip(&mut out) {
                                *result = snapshot.locate_local(id).unwrap();
                            }
                            let ns = start.elapsed().as_nanos();
                            ready.wait();
                            assert_eq!(&out, expected);
                            ns
                        })
                    })
                    .collect();
                ready.wait();
                let start = Instant::now();
                ready.wait();
                ready.wait();
                let wall_ns = start.elapsed().as_nanos();
                for h in handles {
                    h.join().unwrap();
                }
                wall_ns
            });
            println!(
                "{}",
                json!({"kind":"catalog_concurrent_readers","rows":count,"workers":workers,
                "queries_per_worker":ids.len(),"wall_ns":wall_ns,"cache":"warm_os_and_trace",
                "full_output_verified":true,"writer_concurrency_tested":false})
            );
        }
        assert_eq!(
            accounting::inventory(store.root()).unwrap(),
            before_query,
            "queries mutated storage"
        );
        std::fs::write(
            temp.path().join("catalog-reference.json"),
            serde_json::to_vec(&json!({
            "reference":catalog.root().unwrap().unwrap(),"packs":packs,"rows":count}))
            .unwrap(),
        )
        .unwrap();
        drop(snapshot);
        drop(catalog);
        drop(publisher);
        drop(old);
        drop(file);
        for eviction in ["warm", "evict-private-files"] {
            assert!(std::process::Command::new(std::env::current_exe().unwrap())
                .arg("--reopen-catalog")
                .arg(temp.path())
                .arg(eviction)
                .status()
                .unwrap()
                .success());
        }
    }
    all_passed
}

pub fn reopen(root: &Path, eviction: bool) {
    let meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("catalog-reference.json")).unwrap())
            .unwrap();
    let reference: ContentRef = serde_json::from_value(meta["reference"].clone()).unwrap();
    let packs: Vec<ContentRef> = serde_json::from_value(meta["packs"].clone()).unwrap();
    let count = meta["rows"].as_u64().unwrap() as usize;
    if eviction {
        let mut pending = vec![root.join("objects")];
        while let Some(path) = pending.pop() {
            for entry in std::fs::read_dir(path).unwrap() {
                let entry = entry.unwrap();
                let kind = entry.file_type().unwrap();
                if kind.is_dir() {
                    pending.push(entry.path());
                } else if kind.is_file() {
                    let file = std::fs::File::open(entry.path()).unwrap();
                    file.sync_all().unwrap();
                    rustix::fs::fadvise(&file, 0, None, rustix::fs::Advice::DontNeed).unwrap();
                }
            }
        }
    }
    let phase = Phase::start();
    let store = LocalCas::open_shared(root.join("objects")).unwrap();
    let catalog = UnifiedCatalogIndex::open_local(store, &reference).unwrap();
    let snapshot = catalog.snapshot().unwrap();
    phase.finish(
        "hash",
        if eviction {
            "fresh-process-open-eviction-requested"
        } else {
            "fresh-process-open-warm-os"
        },
        count,
    );
    let ids: Vec<_> = (0..10_000usize)
        .map(|n| {
            id(if n % 5 == 0 {
                count + n
            } else {
                n * 7919 % count
            })
        })
        .collect();
    let mut actual = vec![None; ids.len()];
    let start = Instant::now();
    for (id, out) in ids.iter().zip(&mut actual) {
        *out = snapshot.locate_local(id).unwrap();
    }
    let wall_ns = start.elapsed().as_nanos();
    for (i, (id, out)) in ids.iter().zip(&actual).enumerate() {
        let n = i * 7919 % count;
        assert_eq!(
            *out,
            (i % 5 != 0).then(|| LocatedContent {
                content: ContentRef {
                    id: id.clone(),
                    size_bytes: n as u64 + 1
                },
                location: PhysicalCandidate::PackMember {
                    pack: packs[n / 500_000].clone()
                },
            })
        );
    }
    println!(
        "{}",
        json!({"kind":"catalog_fresh_process_query","rows":count,"queries":ids.len(),"wall_ns":wall_ns,
        "eviction_requested":eviction,"cold_cache_proven":false,"cache":"open may fault or prewarm pages; no cold guarantee",
        "full_output_verified":true})
    );
}
