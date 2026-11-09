//! Fixed pure exact-search shootout. Payloads and readers do not exist here.
#[path = "search_index_bench/accounting.rs"]
mod accounting;
#[path = "search_index_bench/catalog.rs"]
mod catalog;
#[path = "search_index_bench/cold.rs"]
mod cold;
#[path = "search_index_bench/concurrency.rs"]
mod concurrency;
#[path = "search_index_bench/direct.rs"]
mod direct;
#[path = "search_index_bench/foundation.rs"]
mod foundation;
#[path = "search_index_bench/growth.rs"]
mod growth;
#[path = "search_index_bench/matrix.rs"]
mod matrix;
#[path = "search_index_bench/paging.rs"]
mod paging;
#[path = "search_index_bench/snapshot.rs"]
mod snapshot;
use polar_cas::{
    shared::{LocatedContent, PhysicalCandidate},
    ContentRef, ObjectId,
};
use polar_cas_local::shared_index::{SearchIndex, SearchLayout};
use ptr_hash::{hash::FxHash, FastPtrHash, PtrHashParams};
use serde_json::json;
use std::{hint::black_box, sync::atomic::AtomicBool, time::Instant};

fn id(n: usize) -> ObjectId {
    ObjectId::for_bytes(&n.to_le_bytes())
}
fn token(row: &Option<LocatedContent>) -> u64 {
    // Require the whole returned identity/location to exist, including Pack ID;
    // inlined competitors must not discard fields merely unused by the checksum.
    let row = black_box(row);
    row.as_ref().map_or(0, |r| {
        r.content.size_bytes
            ^ if matches!(r.location, PhysicalCandidate::Loose) {
                1
            } else {
                2
            }
    })
}
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--writer-process") if args.len() == 3 => {
            concurrency::worker(std::path::Path::new(&args[1]), args[2].parse().unwrap());
            return;
        }
        Some("--reopen-values") if args.len() == 2 => {
            foundation::reopen(std::path::Path::new(&args[1]));
            return;
        }
        Some("--reopen-catalog") if args.len() == 3 => {
            assert!(matches!(args[2].as_str(), "warm" | "evict-private-files"));
            catalog::reopen(
                std::path::Path::new(&args[1]),
                args[2] == "evict-private-files",
            );
            return;
        }
        None | Some("--legacy-matrix") => {}
        _ => panic!("expected no arguments or --legacy-matrix"),
    }
    let executable = std::env::current_exe().unwrap();
    let binary_digest =
        ObjectId::for_reader(&mut std::fs::File::open(&executable).unwrap()).unwrap();
    println!(
        "{}",
        json!({"kind":"binary","digest":binary_digest.to_string(),"profile":"root release opt-level=s","cpuinfo":std::fs::read_to_string("/proc/cpuinfo").unwrap().lines().find(|l|l.starts_with("model name")),"available_parallelism":std::thread::available_parallelism().unwrap().get()})
    );
    let stop = AtomicBool::new(false);
    let root = std::path::Path::new("/data/polar/cache/product-test-runtime");
    if args.is_empty() {
        let hash_passed = catalog::run(root);
        paging::run(root);
        let lifecycle_passed = foundation::run(root);
        concurrency::run(root);
        println!(
            "{}",
            json!({"kind":"foundation_summary","hash_warm_gate_passed":hash_passed,
            "space_gate_passed":lifecycle_passed,"coverage":"layers 1-3 partial; not whole IDEA-CAVE",
            "pending":["unmanaged and domain history retention", "verified cold cache", "complete remote candidates", "performance under crash injection"]})
        );
        assert!(
            hash_passed && lifecycle_passed,
            "foundation benchmark gates failed; see structured samples"
        );
        return;
    }
    let temp = tempfile::Builder::new()
        .prefix("cas-search-shootout-")
        .tempdir_in(root)
        .unwrap();
    println!(
        "{}",
        json!({"kind":"conditions","unix_millis":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis(),"complete_output_black_box":true,"rows":[1048576,10485760],"queries":100000,"rounds":8,"batch":1024,"payload_io":false,"output_dir":temp.path()})
    );
    for n in [1048576, 10485760] {
        let start = Instant::now();
        let pack = ContentRef {
            id: id(usize::MAX),
            size_bytes: 123456,
        };
        let mut rows: Vec<_> = (0..n)
            .map(|k| LocatedContent {
                content: ContentRef {
                    id: id(k),
                    size_bytes: (k + 100) as u64,
                },
                location: if k.is_multiple_of(2) {
                    PhysicalCandidate::Loose
                } else {
                    PhysicalCandidate::PackMember { pack: pack.clone() }
                },
            })
            .collect();
        rows.sort_unstable_by_key(|r| *r.content.id.digest_bytes());
        let keys: Vec<_> = rows.iter().map(|r| *r.content.id.digest_bytes()).collect();
        let build = Instant::now();
        let ph = FastPtrHash::<FxHash, [u8; 32]>::new(&keys, PtrHashParams::default_fast());
        let ph_ns = build.elapsed().as_nanos();
        let mut map = hashbrown::HashTable::with_capacity(n);
        let h = |key: &[u8; 32]| u64::from_le_bytes(key[..8].try_into().unwrap());
        for (i, k) in keys.iter().enumerate() {
            map.insert_unique(h(k), i as u64, |row| h(&keys[*row as usize]));
        }
        println!(
            "{}",
            json!({"kind":"prepared","rows":n,"all_prepare_ns":start.elapsed().as_nanos(),"ph_build_ns":ph_ns,"ph_slots":ph.max_index()})
        );
        let db = temp.path().join(format!("locations-{n}.sqlite"));
        let db_start = Instant::now();
        matrix::sqlite(&db, &rows);
        println!(
            "{}",
            json!({"kind":"sqlite","rows":n,"bytes":std::fs::metadata(&db).unwrap().len(),"build_ns":db_start.elapsed().as_nanos(),"version":rusqlite::version()})
        );
        let native_path = temp.path().join(format!("native-{n}.idx"));
        let mut native_file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&native_path)
            .unwrap();
        let start = Instant::now();
        let (native_digest, native_count) = SearchIndex::build_fast(
            &mut native_file,
            rows.iter().cloned().map(Ok),
            temp.path(),
            &stop,
        )
        .unwrap();
        assert_eq!(native_count, n as u64);
        let build_ns = start.elapsed().as_nanos();
        let start = Instant::now();
        let native = SearchIndex::open(&native_file, Some(&native_digest)).unwrap();
        println!(
            "{}",
            json!({"kind":"native_index","rows":n,"layout":"phf64","build_ns":build_ns,"open_ns":start.elapsed().as_nanos(),"bytes":native.file_bytes(),"digest":native_digest.to_string(),"memory":std::fs::read_to_string("/proc/self/status").unwrap().lines().filter(|l|l.starts_with("VmRSS:")||l.starts_with("VmHWM:")).collect::<Vec<_>>() })
        );
        for (layout, label) in [
            (SearchLayout::Compact48, "compact48"),
            (SearchLayout::Aligned64, "aligned64"),
        ] {
            let mut file = std::fs::File::create(temp.path().join(label)).unwrap();
            drop(file);
            file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(temp.path().join(label))
                .unwrap();
            let start = Instant::now();
            let (digest, count) =
                SearchIndex::build(&mut file, rows.iter().cloned().map(Ok), layout, &stop).unwrap();
            assert_eq!(count, n as u64);
            let index = SearchIndex::open(&file, Some(&digest)).unwrap();
            println!(
                "{}",
                json!({"kind":"index","rows":n,"layout":label,"build_ns":start.elapsed().as_nanos(),"bytes":index.file_bytes(),"digest":digest.to_string()})
            );
            let direct = direct::DirectRecords::build(
                &temp.path().join(format!("ph-{n}-{label}")),
                if label == "compact48" { 48 } else { 64 },
                &rows,
                &ph,
                &pack,
            );
            println!(
                "{}",
                json!({"kind":"ph_records","rows":n,"layout":label,"bytes":direct.file_bytes()})
            );
            for case in ["loose", "pack", "miss", "mixed"] {
                let source: Vec<_> = (0..100000)
                    .map(|q| {
                        let at = q * 7919 % n;
                        match case {
                            "loose" => at & !1,
                            "pack" => at | 1,
                            "miss" => n + at,
                            _ => {
                                if q % 5 == 4 {
                                    n + at
                                } else {
                                    at
                                }
                            }
                        }
                    })
                    .collect();
                let queries: Vec<_> = source.iter().map(|k| id(*k)).collect();
                let expected = source.iter().fold(0u64, |a, k| {
                    a.wrapping_add(if *k < n {
                        ((*k + 100) as u64) ^ if k.is_multiple_of(2) { 1 } else { 2 }
                    } else {
                        0
                    })
                });
                let trace = ObjectId::for_bytes(
                    &queries
                        .iter()
                        .flat_map(|q| q.digest_bytes().iter().copied())
                        .collect::<Vec<_>>(),
                );
                let methods = [
                    "binary",
                    "binary_batch",
                    "sorted_binary",
                    "hash",
                    "batch8",
                    "batch16",
                    "batch32",
                    "batch64",
                    "hashbrown",
                    "ptr_scalar",
                    "ptr_stream",
                    "native_scalar",
                    "native_batch8",
                    "native_batch16",
                    "native_batch32",
                    "native_batch64",
                ];
                let mut output = vec![None; 1024];
                let mut slots = Vec::with_capacity(1024);
                let mut sorted = Vec::with_capacity(1024);
                for q in &queries {
                    assert_eq!(native.locate(q).unwrap(), index.locate(q).unwrap());
                    assert_eq!(
                        direct.locate(ph.index(q.digest_bytes()), q),
                        index.locate(q).unwrap()
                    );
                }
                let mut run = |method: &str| -> u64 {
                    let mut sum = 0u64;
                    for chunk in queries.chunks(1024) {
                        match method {
                            "binary" => {
                                for q in chunk {
                                    sum = sum.wrapping_add(token(
                                        &index.locate_binary(black_box(q)).unwrap(),
                                    ));
                                }
                            }
                            "binary_batch" => {
                                index
                                    .locate_binary_batch_into(chunk, &mut output[..chunk.len()])
                                    .unwrap();
                                for hit in &output[..chunk.len()] {
                                    sum = sum.wrapping_add(token(hit));
                                }
                            }
                            "sorted_binary" => {
                                sorted.clear();
                                sorted.extend(chunk.iter().enumerate());
                                sorted.sort_unstable_by_key(|(_, q)| *q.digest_bytes());
                                for (i, q) in &sorted {
                                    output[*i] = index.locate_binary(q).unwrap();
                                }
                                for hit in &output[..chunk.len()] {
                                    sum = sum.wrapping_add(token(hit));
                                }
                            }
                            "hash" => {
                                for q in chunk {
                                    sum = sum
                                        .wrapping_add(token(&index.locate(black_box(q)).unwrap()));
                                }
                            }
                            "native_scalar" => {
                                for q in chunk {
                                    sum = sum.wrapping_add(token(&native.locate(q).unwrap()));
                                }
                            }
                            "native_batch8" | "native_batch16" | "native_batch32"
                            | "native_batch64" => {
                                let out = &mut output[..chunk.len()];
                                match method {
                                    "native_batch8" => native.locate_batch_into::<8>(chunk, out),
                                    "native_batch16" => native.locate_batch_into::<16>(chunk, out),
                                    "native_batch32" => native.locate_batch_into::<32>(chunk, out),
                                    _ => native.locate_batch_into::<64>(chunk, out),
                                }
                                .unwrap();
                                for hit in out {
                                    sum = sum.wrapping_add(token(hit));
                                }
                            }
                            "hashbrown" => {
                                for q in chunk {
                                    let k = q.digest_bytes();
                                    let hit = map
                                        .find(h(k), |row| keys[*row as usize] == *k)
                                        .map(|row| index.row(*row).unwrap());
                                    sum = sum.wrapping_add(token(&hit));
                                }
                            }
                            "ptr_scalar" => {
                                for q in chunk {
                                    let hit = direct.locate(ph.index(q.digest_bytes()), q);
                                    sum = sum.wrapping_add(token(&hit));
                                }
                            }
                            "ptr_stream" => {
                                slots.clear();
                                ph.index_stream::<32, _>(chunk.iter().map(|q| q.digest_bytes()))
                                    .for_each(|slot| {
                                        direct.prefetch(slot);
                                        slots.push(slot);
                                    });
                                for (q, slot) in chunk.iter().zip(&slots) {
                                    sum = sum.wrapping_add(token(&direct.locate(*slot, q)));
                                }
                            }
                            _ => {
                                let out = &mut output[..chunk.len()];
                                match method {
                                    "batch8" => index.locate_batch_into::<8>(chunk, out),
                                    "batch16" => index.locate_batch_into::<16>(chunk, out),
                                    "batch32" => index.locate_batch_into::<32>(chunk, out),
                                    "batch64" => index.locate_batch_into::<64>(chunk, out),
                                    _ => unreachable!(),
                                }
                                .unwrap();
                                for r in out {
                                    sum = sum.wrapping_add(token(r));
                                }
                            }
                        }
                    }
                    black_box(sum)
                };
                for method in methods {
                    assert_eq!(run(method), expected, "preflight {method}");
                }
                for sample in 0..8 {
                    let order: Vec<_> = if sample % 2 == 0 {
                        methods.into_iter().collect()
                    } else {
                        methods.into_iter().rev().collect()
                    };
                    for method in order {
                        let start = Instant::now();
                        let got = run(method);
                        let ns = start.elapsed().as_nanos();
                        assert_eq!(got, expected);
                        println!(
                            "{}",
                            json!({"kind":"search","rows":n,"layout":label,"case":case,"method":method,"sample":sample,"queries":queries.len(),"batch":1024,"wall_ns":ns,"trace":trace.to_string(),"checksum":got})
                        );
                    }
                }
            }
            if label == "aligned64" {
                matrix::run(&index, &native, &db, n, &pack);
            }
        }
        snapshot::run(temp.path(), &native_path, &native_digest, n);
        drop(native);
        let grouped_file = std::fs::File::open(temp.path().join("aligned64")).unwrap();
        cold::run(&grouped_file, &native_file, n);
    }
    growth::run(temp.path());
}
