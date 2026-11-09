//! Real processes exercising the existing shared publication lock and head.
use super::accounting::{self, Phase};
use polar_cas::{
    catalog::writers::{SaveAttribution, WriterId, WriterRecord},
    source::LocationId,
    ContentRef, ObjectId,
};
use polar_cas_local::{catalog_index::UnifiedCatalogIndex, LocalCas};
use serde_json::json;
use std::{
    path::Path,
    process::{Child, Command},
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

const ROUNDS: usize = 4;
const BATCH: usize = 8;
fn writer(n: usize) -> WriterId {
    WriterId::new(format!("{n:064x}")).unwrap()
}
fn records(worker: usize, round: usize) -> Vec<WriterRecord> {
    (0..BATCH)
        .map(|n| WriterRecord {
            content: ContentRef {
                id: ObjectId::for_bytes(
                    format!("worker:{worker}:round:{round}:item:{n}").as_bytes(),
                ),
                size_bytes: 1,
            },
            attribution: SaveAttribution {
                writer: writer(worker),
                store: None,
                scope: None,
            },
            destination: LocationId::new("concurrent-fixture").unwrap(),
            first_saved_at_ms: 1,
        })
        .collect()
}
struct Workers(Vec<Child>);
impl Drop for Workers {
    fn drop(&mut self) {
        for child in &mut self.0 {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
    }
}
pub fn run(root: &Path) {
    for workers in [1usize, 8, 32] {
        let temp = tempfile::Builder::new()
            .prefix("index-writers-")
            .tempdir_in(root)
            .unwrap();
        let store = LocalCas::open_shared(temp.path().join("objects")).unwrap();
        let catalog = UnifiedCatalogIndex::open_shared(store.clone()).unwrap();
        let original = catalog.snapshot().unwrap();
        let mut children = Workers(
            (0..workers)
                .map(|worker| {
                    Command::new(std::env::current_exe().unwrap())
                        .arg("--writer-process")
                        .arg(temp.path())
                        .arg(worker.to_string())
                        .spawn()
                        .unwrap()
                })
                .collect(),
        );
        let deadline = Instant::now() + Duration::from_secs(120);
        while !(0..workers).all(|i| temp.path().join(format!("ready-{i}")).exists()) {
            assert!(Instant::now() < deadline, "workers did not become ready");
            std::thread::sleep(Duration::from_millis(10));
        }
        let peak = accounting::PeakSampler::start(store.root());
        let start = Instant::now();
        std::fs::write(temp.path().join("go"), []).unwrap();
        let mut observed = 0;
        let mut refresh_samples = Vec::new();
        loop {
            assert!(Instant::now() < deadline, "concurrent writers timed out");
            let query_start = Instant::now();
            catalog.refresh().unwrap();
            let snapshot = catalog.snapshot().unwrap();
            let count = snapshot.writer_record_count();
            refresh_samples.push(query_start.elapsed().as_nanos());
            assert!(
                count >= observed && count.is_multiple_of(BATCH as u64),
                "partial or regressing publication"
            );
            observed = count;
            assert_eq!(original.writer_record_count(), 0, "old snapshot changed");
            if children
                .0
                .iter_mut()
                .all(|child| child.try_wait().unwrap().is_some())
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let wall_ns = start.elapsed().as_nanos();
        for child in &mut children.0 {
            assert!(child.wait().unwrap().success());
        }
        catalog.refresh().unwrap();
        let final_snapshot = catalog.snapshot().unwrap();
        assert_eq!(
            final_snapshot.writer_record_count(),
            (workers * ROUNDS * BATCH) as u64
        );
        for worker in 0..workers {
            let mut expected: Vec<_> = (0..ROUNDS)
                .flat_map(|round| records(worker, round))
                .map(|r| r.content)
                .collect();
            expected.sort_unstable_by_key(|c| c.id.to_string());
            assert_eq!(
                final_snapshot
                    .writer_contents(&writer(worker), None, 201)
                    .unwrap(),
                expected
            );
        }
        peak.finish(
            "publication",
            "concurrent-writers",
            workers * ROUNDS * BATCH,
        );
        accounting::space(
            "publication",
            "after-concurrent-writes",
            workers * ROUNDS * BATCH,
            store.root(),
        );
        println!(
            "{}",
            json!({"kind":"concurrent_publication","workers":workers,"processes":true,
            "rounds_per_worker":ROUNDS,"records_per_commit":BATCH,"wall_ns":wall_ns,
            "refresh_and_snapshot_samples_ns":refresh_samples,"full_output_verified":true,
            "old_snapshot_preserved":true,"atomic_batch_counts":true,"lost_updates":0})
        );
    }
}

pub fn worker(root: &Path, worker: usize) {
    let store = LocalCas::open_shared(root.join("objects")).unwrap();
    let catalog = UnifiedCatalogIndex::open_shared(store).unwrap();
    std::fs::write(root.join(format!("ready-{worker}")), []).unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    while !root.join("go").exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    let stop = AtomicBool::new(false);
    let phase = Phase::start();
    for round in 0..ROUNDS {
        let records = records(worker, round);
        let start = Instant::now();
        let publication = catalog.begin_writer_publication(&stop, deadline).unwrap();
        let acquire_refresh_ns = start.elapsed().as_nanos();
        let start = Instant::now();
        publication.record_writes(&records).unwrap();
        drop(publication);
        let publish_ns = start.elapsed().as_nanos();
        println!(
            "{}",
            json!({"kind":"writer_commit","worker":worker,"round":round,
            "acquire_and_refresh_ns":acquire_refresh_ns,"publish_and_unlock_ns":publish_ns})
        );
    }
    phase.finish("publication", "worker-four-commits", ROUNDS * BATCH);
}
