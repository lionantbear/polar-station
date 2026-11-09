//! Same data and binary codec as the versions experiment; compare publication
//! policies, including maintenance IO, without rebuilding the experimental hash leaf.
use super::*;
use crate::shared_index::{lifecycle::ManagedIndexHead, partitions::write_metrics};
#[allow(dead_code)]
#[path = "../../../../../examples/search_index_bench/accounting.rs"]
mod accounting;

fn sum(
    total: &mut BTreeMap<&'static str, write_metrics::Stage>,
    current: BTreeMap<&'static str, write_metrics::Stage>,
) {
    for (key, value) in current {
        let t = total.entry(key).or_default();
        t.calls += value.calls;
        t.nanos += value.nanos;
        t.bytes += value.bytes;
        t.records += value.records;
    }
}
fn run(versions: usize, shared: usize, deferred: bool, count: usize) -> serde_json::Value {
    let mut model = Versions::build(count, versions, shared);
    let head = ManagedIndexHead::open(
        model.store.clone(),
        &model.store.root().join(".experiment-head"),
    )
    .unwrap();
    head.select(None, &model.snapshot).unwrap();
    let retained = model.snapshot.clone();
    let before = accounting::inventory(model.store.root()).unwrap();
    let sampler = accounting::PeakSampler::start(model.store.root());
    let mut total = BTreeMap::new();
    let mut latencies = vec![];
    let changed = 100.min(count / versions);
    let mut prepare_ns = 0;
    for batch in 0..12 {
        let started = Instant::now();
        let mut changes = vec![];
        for n in 0..changed {
            let id = uuid(n);
            let bytes = forward(&model.snapshot, &id).unwrap();
            let mut hashes = page(&bytes, 0, list(&bytes).0);
            hashes.push(fixture_id(count + batch * changed + n));
            changes.push(forward_record(id, &mut hashes));
        }
        prepare_ns += started.elapsed().as_nanos();
        write_metrics::start();
        let started = Instant::now();
        let next = if deferred {
            ValueSnapshot::publish_deferred(
                &model.store,
                Some(&model.snapshot),
                &changes,
                json!({}),
            )
        } else {
            ValueSnapshot::publish(&model.store, Some(&model.snapshot), &changes, json!({}))
        }
        .unwrap();
        head.select(Some(model.snapshot.reference()), &next)
            .unwrap();
        latencies.push(started.elapsed().as_nanos());
        sum(&mut total, write_metrics::take());
        model.snapshot = next;
        for n in 0..changed {
            let old = forward(&retained, &uuid(n)).unwrap();
            let current = forward(&model.snapshot, &uuid(n)).unwrap();
            assert_eq!(list(&current).0, list(&old).0 + batch + 1);
            let all = page(&current, 0, list(&current).0);
            for previous in 0..=batch {
                assert!(all.contains(&fixture_id(count + previous * changed + n)));
            }
            for hash in page(&old, 0, list(&old).0) {
                assert!(all.contains(&hash));
            }
        }
    }
    let peaks = sampler.finish(
        "uuid-write06",
        if deferred { "deferred" } else { "eager" },
        count,
    );
    let retained_space = accounting::inventory(model.store.root()).unwrap();
    let final_before = model.snapshot.clone();
    write_metrics::start();
    let started = Instant::now();
    let compact = model.snapshot.compact().unwrap();
    head.select(Some(model.snapshot.reference()), &compact)
        .unwrap();
    let compact_ns = started.elapsed().as_nanos();
    let mut maintenance = write_metrics::take();
    assert!(model.snapshot.same_records(&compact).unwrap());
    model.snapshot = compact;
    drop(retained);
    drop(final_before);
    write_metrics::start();
    let started = Instant::now();
    let collected = model.store.collect_index_artifacts().unwrap();
    let collect_ns = started.elapsed().as_nanos();
    sum(&mut maintenance, write_metrics::take());
    let steady = accounting::inventory(model.store.root()).unwrap();
    // Same head/registry/control-file accounting as the updated store.
    let reference_dir = tempfile::tempdir_in(model.fixture._directory.path()).unwrap();
    let reference_store = LocalCas::open_shared(reference_dir.path().join("reference")).unwrap();
    let mut facts = vec![];
    model
        .snapshot
        .visit_records(|row| {
            facts.push(ValueChange::Put(row));
            Ok(())
        })
        .unwrap();
    let reference = ValueSnapshot::publish(&reference_store, None, &facts, json!({})).unwrap();
    let reference_head = ManagedIndexHead::open(
        reference_store.clone(),
        &reference_store.root().join(".experiment-head"),
    )
    .unwrap();
    reference_head.select(None, &reference).unwrap();
    reference_store.collect_index_artifacts().unwrap();
    let tight = accounting::inventory(reference_store.root()).unwrap();
    let ratio = steady.unique_logical_bytes as f64 / tight.unique_logical_bytes as f64;
    assert!(ratio <= 1.25, "steady-space ratio {ratio}");
    assert!(model.snapshot.same_records(&reference).unwrap());
    assert_eq!(
        model.snapshot.compact().unwrap().reference(),
        model.snapshot.reference()
    );
    let mut query = vec![];
    for _ in 0..5 {
        query.push(sample(
            |n| {
                let bytes = forward(&model.snapshot, &uuid(n % (count / versions))).unwrap();
                black_box(list(&bytes).0);
            },
            8192,
        ));
    }
    let writes = |m: &BTreeMap<&str, write_metrics::Stage>| {
        [
            "prepare_write",
            "segment_write",
            "root_write",
            "head_write_sync",
            "registry_write_sync",
        ]
        .iter()
        .map(|k| m.get(k).map_or(0, |m| m.bytes))
        .sum::<u64>()
    };
    json!({"kind":"uuid_write06", "hashes":count, "versions":versions, "shared_percent":shared, "deferred":deferred,
        "batches":12, "changes_per_batch":changed, "prepare_ns":prepare_ns, "commit_ns":latencies,
        "foreground_metrics":total,"maintenance_metrics":maintenance,
        "foreground_written_bytes":writes(&total),"maintenance_written_bytes":writes(&maintenance),
        "total_written_bytes":writes(&total)+writes(&maintenance),"compact_ns":compact_ns,"collect_ns":collect_ns,
        "collected":collected,"before":before,"retained_space":retained_space,"steady":steady,"compact_reference":tight,
        "steady_ratio":ratio,"sampled_peaks":peaks,"query_count_ns":query,
        "scope":"UUID ValueSnapshot plus managed head, registry and control files; hash leaf and source payload excluded",
        "write_accounting":"native preparation + final segment/root + head/registry write_all bytes; not filesystem/device write amplification; sync counts/times separate",
        "comparison":"same current binary, eager versus deferred policy; sampling enabled during writes; no interim collection"})
}
#[test]
fn deferred_version_updates_roundtrip_and_reclaim() {
    for deferred in [false, true] {
        run(10, 1, deferred, 1000);
    }
}
#[test]
#[ignore = "Release-only write amplification experiment through Host executor"]
fn uuid_deferred_write_release_experiment() {
    assert!(!black_box(cfg!(debug_assertions)));
    let digest =
        ObjectId::for_reader(&mut File::open(std::env::current_exe().unwrap()).unwrap()).unwrap();
    println!(
        "{}",
        json!({"kind":"uuid_write06_conditions","binary":digest.to_string(),"cpu_pinned":false,"peak_interval_ms":100,"profile":"release"})
    );
    for versions in [1, 10, 100] {
        for shared in [0, 1] {
            for deferred in [false, true] {
                println!("{}", run(versions, shared, deferred, 100_000));
            }
        }
    }
}

#[test]
#[ignore = "Release-only paired query verification through Host executor"]
fn uuid_deferred_paired_query_release_experiment() {
    assert!(!black_box(cfg!(debug_assertions)));
    println!(
        "{}",
        json!({"kind":"uuid_write06_paired_conditions","binary":ObjectId::for_reader(&mut File::open(std::env::current_exe().unwrap()).unwrap()).unwrap().to_string(),"cache":"warm","rounds":9,"queries":16384,"passes":8,"order":"alternating AB/BA","cpu_pinned":true,"key_digest":"precomputed outside query timing"})
    );
    for versions in [1, 10, 100] {
        for shared in [0, 1] {
            let model = Versions::build(100_000, versions, shared);
            let mut eager = model.snapshot.clone();
            let mut deferred = model.snapshot.clone();
            for batch in 0..12 {
                let changes: Vec<_> = (0..100)
                    .map(|n| {
                        let id = uuid(n);
                        let bytes = forward(&eager, &id).unwrap();
                        let mut hashes = page(&bytes, 0, list(&bytes).0);
                        hashes.push(fixture_id(100_000 + batch * 100 + n));
                        forward_record(id, &mut hashes)
                    })
                    .collect();
                eager = ValueSnapshot::publish(&model.store, Some(&eager), &changes, json!({}))
                    .unwrap();
                deferred = ValueSnapshot::publish_deferred(
                    &model.store,
                    Some(&deferred),
                    &changes,
                    json!({}),
                )
                .unwrap();
            }
            deferred = deferred.compact().unwrap();
            assert!(eager.same_records(&deferred).unwrap());
            let queries: Vec<_> = (0..16384)
                .map(|n| {
                    let id = if n % 16 == 0 {
                        Uuid::nil()
                    } else {
                        uuid((n * 17) % (100_000 / versions))
                    };
                    (id, ObjectId::for_bytes(id.as_bytes()))
                })
                .collect();
            let _cpu = QueryCpu::pin();
            for operation in ["count", "first_page", "last_page"] {
                let mut ratios = vec![];
                for round in 0..9 {
                    let mut times = [0.; 2];
                    for mode in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
                        let snapshot = if mode == 0 { &eager } else { &deferred };
                        times[mode] = sample(
                            |n| {
                                let (id, locator) = &queries[n];
                                if let Some(bytes) = snapshot.get(locator, id.as_bytes()).unwrap() {
                                    let (count, _) = list(&bytes);
                                    match operation {
                                        "count" => {
                                            black_box(count);
                                        }
                                        "first_page" => {
                                            black_box(page(&bytes, 0, 8));
                                        }
                                        _ => {
                                            black_box(page(&bytes, count.saturating_sub(8), 8));
                                        }
                                    }
                                } else {
                                    black_box(false);
                                }
                            },
                            queries.len(),
                        );
                    }
                    ratios.push(times[1] / times[0]);
                    println!(
                        "{}",
                        json!({"kind":"uuid_write06_paired_query","versions":versions,"shared_percent":shared,"operation":operation,"round":round,"eager_ns":times[0],"deferred_ns":times[1],"ratio":times[1]/times[0]})
                    );
                }
                ratios.sort_by(f64::total_cmp);
                println!(
                    "{}",
                    json!({"kind":"uuid_write06_paired_gate","versions":versions,"shared_percent":shared,"operation":operation,"median_ratio":ratios[4],"threshold":1.05,"passed":ratios[4]<=1.05})
                );
                assert!(
                    ratios[4] <= 1.05,
                    "paired query gate failed: {versions}/{shared}/{operation}: {}",
                    ratios[4]
                );
            }
        }
    }
}

#[test]
fn deferred_retraction_preserves_empty_identity_and_other_binding() {
    let model = Versions::build(1000, 10, 1);
    let first = uuid(0);
    let other = uuid(1);
    let shared_hash = fixture_id(0);
    let bytes = forward(&model.snapshot, &first).unwrap();
    let mut hashes = page(&bytes, 0, list(&bytes).0);
    assert!(hashes.contains(&shared_hash));
    hashes.retain(|h| h != &shared_hash);
    let removed = ValueSnapshot::publish_deferred(
        &model.store,
        Some(&model.snapshot),
        &[forward_record(first, &mut hashes)],
        json!({}),
    )
    .unwrap();
    let other_bytes = forward(&removed, &other).unwrap();
    assert!(page(&other_bytes, 0, list(&other_bytes).0).contains(&shared_hash));
    let changes = [forward_record(first, &mut vec![])];
    let empty =
        ValueSnapshot::publish_deferred(&model.store, Some(&removed), &changes, json!({})).unwrap();
    assert_eq!(list(&forward(&empty, &first).unwrap()).0, 0);
    assert!(forward(&empty, &Uuid::nil()).is_none());
    assert_eq!(
        ValueSnapshot::publish_deferred(&model.store, Some(&empty), &changes, json!({}))
            .unwrap()
            .reference(),
        empty.reference()
    );
    assert!(empty.same_records(&empty.compact().unwrap()).unwrap());
}
