//! Exercise the published-generation facade as well as the navigator itself.
use super::*;
use polar_cas::shared::{IndexRun, SharedIndexGeneration};
use polar_cas_local::{shared_index::SharedPackStore, LocalCas};
pub fn run(root: &std::path::Path, path: &std::path::Path, digest: &ObjectId, n: usize) {
    let store =
        SharedPackStore::open(LocalCas::open_shared(root.join(format!("snapshot-{n}"))).unwrap())
            .unwrap();
    let metadata = store.root().join(".polar-pack-index-v1");
    std::fs::hard_link(path, metadata.join(format!("runs/{}.idx", digest.hex()))).unwrap();
    let generation = SharedIndexGeneration {
        version: 3,
        runs: vec![IndexRun {
            index: digest.clone(),
            rows: n as u64,
            level: n.ilog2(),
        }],
        ..Default::default()
    };
    let bytes = polar_cas::canonical::canonical_json_bytes(&generation).unwrap();
    let id = ObjectId::for_bytes(&bytes);
    std::fs::write(
        metadata.join(format!("generations/{}.json", id.hex())),
        bytes,
    )
    .unwrap();
    std::fs::write(metadata.join("current"), id.hex()).unwrap();
    let view = store.snapshot().unwrap();
    let source: Vec<_> = (0..100000)
        .map(|q| {
            let at = q * 7919 % n;
            if q % 5 == 4 {
                n + at
            } else {
                at
            }
        })
        .collect();
    let queries: Vec<_> = source.iter().map(|k| super::id(*k)).collect();
    let expected = source.iter().fold(0u64, |sum, k| {
        sum.wrapping_add(if *k < n {
            ((*k + 100) as u64) ^ if k.is_multiple_of(2) { 1 } else { 2 }
        } else {
            0
        })
    });
    for batch in [1, 8, 64, 1024] {
        let mut output = vec![None; batch];
        let methods = [
            "snapshot_scalar",
            "cached_scalar",
            "snapshot_batch_into",
            "snapshot_batch_alloc",
            "cached_batch_alloc",
        ];
        let mut ignored_times = Vec::new();
        let mut run = |method: &str| match method {
            "snapshot_scalar" => matrix::chunks::<false, _>(
                &queries,
                batch,
                &mut output,
                &mut ignored_times,
                |chunk, _| {
                    chunk
                        .iter()
                        .fold(0u64, |a, q| a.wrapping_add(token(&view.locate(q).unwrap())))
                },
            ),
            "cached_scalar" => matrix::chunks::<false, _>(
                &queries,
                batch,
                &mut output,
                &mut ignored_times,
                |chunk, _| {
                    chunk.iter().fold(0u64, |a, q| {
                        a.wrapping_add(token(&store.search_snapshot().unwrap().locate(q).unwrap()))
                    })
                },
            ),
            "snapshot_batch_into" => matrix::chunks::<false, _>(
                &queries,
                batch,
                &mut output,
                &mut ignored_times,
                |chunk, out| {
                    view.locate_batch_into(chunk, out).unwrap();
                    out.iter().fold(0u64, |a, v| a.wrapping_add(token(v)))
                },
            ),
            "snapshot_batch_alloc" => matrix::chunks::<false, _>(
                &queries,
                batch,
                &mut output,
                &mut ignored_times,
                |chunk, _| {
                    view.locate_batch(chunk)
                        .unwrap()
                        .iter()
                        .fold(0u64, |a, v| a.wrapping_add(token(v)))
                },
            ),
            "cached_batch_alloc" => matrix::chunks::<false, _>(
                &queries,
                batch,
                &mut output,
                &mut ignored_times,
                |chunk, _| {
                    store
                        .search_snapshot()
                        .unwrap()
                        .locate_batch(chunk)
                        .unwrap()
                        .iter()
                        .fold(0u64, |a, v| a.wrapping_add(token(v)))
                },
            ),
            _ => unreachable!(),
        };
        for method in methods {
            assert_eq!(run(method), expected);
        }
        for sample in 0..8 {
            for step in 0..methods.len() {
                let method = methods[if sample % 2 == 0 {
                    step
                } else {
                    methods.len() - 1 - step
                }];
                let now = Instant::now();
                let sum = run(method);
                let ns = now.elapsed().as_nanos();
                assert_eq!(sum, expected);
                println!(
                    "{}",
                    json!({"kind":"facade","rows":n,"case":"mixed","sample":sample,"method":method,"queries":queries.len(),"batch":batch,"wall_ns":ns,"checksum":sum})
                );
            }
        }
    }
}
