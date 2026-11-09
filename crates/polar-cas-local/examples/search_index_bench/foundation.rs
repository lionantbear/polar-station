//! Layers 2/3: real shared value snapshots; no replacement index or media payload.
use super::accounting::{self, Phase};
use polar_cas::{ContentRef, ObjectId};
use polar_cas_local::{
    shared_index::{
        lifecycle::ManagedIndexHead,
        partitions::{ValueChange, ValueSnapshot},
        values::ValueRecord,
    },
    LocalCas,
};
use serde_json::json;
use std::{hint::black_box, path::Path, time::Instant};

fn record(n: usize, version: u64) -> ValueRecord {
    let key = format!("benchmark-key-{n:016}").into_bytes();
    let mut value = (n as u64).to_le_bytes().to_vec();
    value.extend_from_slice(&version.to_le_bytes());
    value.resize(16 + n % 240, (n % 251) as u8);
    ValueRecord {
        locator: ObjectId::for_bytes(&key),
        key,
        value,
    }
}

fn queries(snapshot: &ValueSnapshot, rows: &[ValueRecord]) {
    let trace: Vec<_> = (0..10_000usize)
        .map(|i| {
            let n = if i % 5 == 0 {
                rows.len() + i
            } else {
                i.wrapping_mul(7919) % rows.len()
            };
            let r = record(n, 0);
            (n, r)
        })
        .collect();
    for (n, r) in &trace {
        assert_eq!(
            snapshot.get(&r.locator, &r.key).unwrap(),
            rows.get(*n)
                .map(|r| std::borrow::Cow::Borrowed(r.value.as_slice()))
        );
    }
    let wrong = &rows[0];
    assert_eq!(
        snapshot
            .get(&wrong.locator, b"different semantic key")
            .unwrap(),
        None
    );
    let mut out = vec![None; 1024];
    // Batches are prepared outside timing; full result equality is checked first.
    let batches: Vec<Vec<_>> = trace
        .chunks(1024)
        .map(|batch| {
            batch
                .iter()
                .map(|(_, r)| (&r.locator, r.key.as_slice()))
                .collect()
        })
        .collect();
    for (batch, wanted) in batches.iter().zip(trace.chunks(1024)) {
        snapshot
            .get_batch_into(batch, &mut out[..batch.len()])
            .unwrap();
        for (actual, (n, _)) in out.iter().zip(wanted) {
            assert_eq!(
                *actual,
                rows.get(*n)
                    .map(|r| std::borrow::Cow::Borrowed(r.value.as_slice()))
            );
        }
    }
    for round in 0..8 {
        let start = Instant::now();
        for (_, r) in &trace {
            black_box(
                snapshot
                    .get(black_box(&r.locator), black_box(&r.key))
                    .unwrap(),
            );
        }
        let single_ns = start.elapsed().as_nanos();
        let start = Instant::now();
        for batch in &batches {
            snapshot
                .get_batch_into(black_box(batch), &mut out[..batch.len()])
                .unwrap();
            black_box(&out[..batch.len()]);
        }
        println!(
            "{}",
            json!({"kind":"value_lookup","rows":rows.len(),"round":round,"queries":trace.len(),
            "single_ns":single_ns,"batch_ns":start.elapsed().as_nanos(),"cache":"warm_os_and_trace",
            "miss_percent":20,"full_output_verified":true})
        );
    }
    let samples = trace
        .iter()
        .map(|(_, r)| {
            let start = Instant::now();
            black_box(
                snapshot
                    .get(black_box(&r.locator), black_box(&r.key))
                    .unwrap(),
            );
            start.elapsed().as_nanos()
        })
        .collect();
    accounting::latency("value", &format!("get-{}", rows.len()), samples);
}

pub fn run(root: &Path) -> bool {
    let mut passed = true;
    for count in [1_000usize, 10_000, 100_000] {
        let temp = tempfile::Builder::new()
            .prefix("index-foundation-")
            .tempdir_in(root)
            .unwrap();
        let store = LocalCas::open_shared(temp.path().join("objects")).unwrap();
        accounting::space("value", "empty", count, store.root());
        let mut rows: Vec<_> = (0..count).map(|n| record(n, 0)).collect();
        let changes: Vec<_> = rows.iter().cloned().map(ValueChange::Put).collect();
        let peak = accounting::PeakSampler::start(store.root());
        let phase = Phase::start();
        let head = ManagedIndexHead::open(store.clone(), &store.root().join("value-head")).unwrap();
        let first = ValueSnapshot::publish(&store, None, &changes, json!({})).unwrap();
        head.select(None, &first).unwrap();
        phase.finish("value", "build", count);
        peak.finish("value", "build", count);
        accounting::active(
            "value",
            first.reference(),
            first.partition_refs(),
            count,
            "built",
        );
        let before = accounting::space("value", "built", count, store.root());
        queries(&first, &rows);
        assert_eq!(
            accounting::inventory(store.root()).unwrap(),
            before,
            "queries mutated the index"
        );
        let phase = Phase::start();
        let noop = ValueSnapshot::publish(&store, Some(&first), &changes, json!({})).unwrap();
        phase.finish("value", "unchanged-full-batch", count);
        assert_eq!(noop.reference(), first.reference());
        assert_eq!(
            accounting::inventory(store.root()).unwrap(),
            before,
            "no-op grew files or bytes"
        );
        drop(noop);
        let mut current = first.clone();
        let peak = accounting::PeakSampler::start(store.root());
        // Keep the original snapshot alive throughout replacement updates.
        for round in 0..12usize {
            let percent = [0usize, 1, 10][round % 3];
            let changed = (count * percent / 100).max(1);
            let changes: Vec<_> = (0..changed)
                .map(|n| {
                    rows[n] = record(n, round as u64 + 1);
                    ValueChange::Put(rows[n].clone())
                })
                .collect();
            let phase = Phase::start();
            let previous = current.reference().clone();
            current = ValueSnapshot::publish(&store, Some(&current), &changes, json!({})).unwrap();
            head.select(Some(&previous), &current).unwrap();
            phase.finish("value", &format!("update-{round}-changed-{changed}"), count);
            assert_eq!(
                first.get(&rows[0].locator, &rows[0].key).unwrap(),
                Some(std::borrow::Cow::Borrowed(record(0, 0).value.as_slice()))
            );
            for row in &rows[..changed] {
                assert_eq!(
                    current.get(&row.locator, &row.key).unwrap(),
                    Some(std::borrow::Cow::Borrowed(row.value.as_slice()))
                );
            }
            accounting::active(
                "value",
                current.reference(),
                current.partition_refs(),
                count,
                &format!("update-{round}"),
            );
            accounting::space(
                "value",
                &format!("update-{round}-old-snapshot-held"),
                count,
                store.root(),
            );
        }
        peak.finish("value", "twelve-updates-old-snapshot-held", count);
        let phase = Phase::start();
        let reopened = ValueSnapshot::open_local(&store, current.reference()).unwrap();
        phase.finish("value", "reopen-warm-os", count);
        for row in &rows {
            assert_eq!(
                reopened.get(&row.locator, &row.key).unwrap(),
                Some(std::borrow::Cow::Borrowed(row.value.as_slice()))
            );
        }
        drop(first);
        drop(reopened);
        let phase = Phase::start();
        let collected = store.collect_index_artifacts().unwrap();
        phase.finish("value", "collect-managed-history", count);
        println!(
            "{}",
            json!({"kind":"index_collection", "rows":count, "receipt":collected})
        );
        let retained = accounting::space(
            "value",
            "old-snapshots-released-collected",
            count,
            store.root(),
        );
        let compact_store = LocalCas::open_shared(temp.path().join("compact")).unwrap();
        let changes: Vec<_> = rows.into_iter().map(ValueChange::Put).collect();
        let phase = Phase::start();
        let compact = ValueSnapshot::publish(&compact_store, None, &changes, json!({})).unwrap();
        let compact_head = ManagedIndexHead::open(
            compact_store.clone(),
            &compact_store.root().join("value-head"),
        )
        .unwrap();
        compact_head.select(None, &compact).unwrap();
        phase.finish("value", "compact-reference-build", count);
        for change in &changes {
            if let ValueChange::Put(row) = change {
                assert_eq!(
                    compact.get(&row.locator, &row.key).unwrap(),
                    current.get(&row.locator, &row.key).unwrap()
                );
            }
        }
        let baseline = accounting::space("value", "compact-reference", count, compact_store.root());
        let ratio = retained.unique_logical_bytes as f64 / baseline.unique_logical_bytes as f64;
        let gate = ratio <= 1.25;
        println!(
            "{}",
            json!({"kind":"index_space_gate","rows":count,"ratio":ratio,"threshold":1.25,
            "passed":gate,"scope":"value-index-only; no business history", "gc":"managed historical roots collected",
            "full_output_verified":true})
        );
        passed &= gate;
        std::fs::write(
            temp.path().join("reference.json"),
            serde_json::to_vec(current.reference()).unwrap(),
        )
        .unwrap();
        drop(current);
        drop(compact);
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--reopen-values")
            .arg(temp.path())
            .status()
            .unwrap();
        assert!(result.success());
    }
    passed
}

pub fn reopen(root: &Path) {
    let reference: ContentRef =
        serde_json::from_slice(&std::fs::read(root.join("reference.json")).unwrap()).unwrap();
    // A real new process, but no host-wide cache eviction: never label this cold.
    let phase = Phase::start();
    let store = LocalCas::open_shared(root.join("objects")).unwrap();
    let snapshot = ValueSnapshot::open_local(&store, &reference).unwrap();
    let count = snapshot.partition_refs().values().map(|r| r.records).sum();
    phase.finish("value", "fresh-process-open-warm-os", count);
    let r = record(0, 0);
    let start = Instant::now();
    let bytes = snapshot.get(&r.locator, &r.key).unwrap().unwrap();
    let query_ns = start.elapsed().as_nanos();
    assert_eq!(&bytes[..8], &0u64.to_le_bytes());
    assert_eq!(&bytes[8..16], &12u64.to_le_bytes());
    println!(
        "{}",
        json!({"kind":"fresh_process_first_query","wall_ns":query_ns,"full_output_verified":true})
    );
}
