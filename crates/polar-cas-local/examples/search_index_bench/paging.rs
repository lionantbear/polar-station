//! Exercise the existing binary fixed-record pager through its real writer API.
use super::accounting::{self, Phase};
use polar_cas::{
    catalog::writers::{SaveAttribution, WriterId, WriterRecord},
    source::LocationId,
    ContentRef, ObjectId,
};
use polar_cas_local::{catalog_index::UnifiedCatalogIndex, LocalCas};
use serde_json::json;
use std::{hint::black_box, path::Path, time::Instant};

pub fn run(root: &Path) {
    for count in [1_000usize, 10_000, 65_536] {
        let temp = tempfile::Builder::new()
            .prefix("index-page-")
            .tempdir_in(root)
            .unwrap();
        let store = LocalCas::open_shared(temp.path().join("objects")).unwrap();
        let catalog = UnifiedCatalogIndex::open_shared(store.clone()).unwrap();
        let writer = WriterId::new("ab".repeat(32)).unwrap();
        let mut rows: Vec<_> = (0..count)
            .map(|n| {
                // Metadata fixture: force one full bucket to expose whole-list work.
                let mut digest = *ObjectId::for_bytes(&(n as u64).to_le_bytes()).digest_bytes();
                digest[0] = 0;
                ContentRef {
                    id: ObjectId::from_sha256(digest),
                    size_bytes: n as u64 + 1,
                }
            })
            .collect();
        rows.sort_unstable_by_key(|r| r.id.to_string());
        let records: Vec<_> = rows
            .iter()
            .map(|content| WriterRecord {
                content: content.clone(),
                attribution: SaveAttribution {
                    writer: writer.clone(),
                    store: None,
                    scope: None,
                },
                destination: LocationId::new("page-fixture").unwrap(),
                first_saved_at_ms: 1,
            })
            .collect();
        let phase = Phase::start();
        for batch in records.chunks(1024) {
            catalog.record_writes(batch).unwrap();
        }
        phase.finish("paging", "fixture-build", count);
        let snapshot = catalog.snapshot().unwrap();
        assert_eq!(snapshot.writer_record_count(), count as u64);
        let before = accounting::space("paging", "built", count, store.root());
        for size in [20usize, 100, 201] {
            for offset in [0usize, count / 2, count] {
                let cursor = offset.checked_sub(1).map(|n| &rows[n].id);
                let expected = &rows[offset..(offset + size).min(count)];
                assert_eq!(
                    snapshot
                        .writer_contents(&writer, cursor, size as u32)
                        .unwrap(),
                    expected
                );
                let samples = (0..1000)
                    .map(|_| {
                        let start = Instant::now();
                        black_box(
                            snapshot
                                .writer_contents(black_box(&writer), black_box(cursor), size as u32)
                                .unwrap(),
                        );
                        start.elapsed().as_nanos()
                    })
                    .collect();
                accounting::latency(
                    "paging",
                    &format!("rows-{count}-offset-{offset}-limit-{size}"),
                    samples,
                );
            }
        }
        // Sequentially traverse every page to detect skips/duplicates and cursor bugs.
        let mut collected = Vec::new();
        loop {
            let page = snapshot
                .writer_contents(&writer, collected.last().map(|r: &ContentRef| &r.id), 201)
                .unwrap();
            if page.is_empty() {
                break;
            }
            collected.extend(page);
            assert!(collected.len() <= rows.len());
        }
        assert_eq!(collected, rows);
        assert_eq!(accounting::inventory(store.root()).unwrap(), before);
        println!(
            "{}",
            json!({"kind":"paging_correctness","rows":count,"full_output_verified":true,
            "query_storage_unchanged":true,"decoded_records":null,
            "note":"runtime decoder counters unavailable here; bounded-decode regression remains in unit suite"})
        );
    }
}
