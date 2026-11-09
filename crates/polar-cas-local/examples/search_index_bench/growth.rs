//! Metadata-only publication/compaction fixture. Pack bodies are synthetic;
//! this measures index maintenance, not EROFS admission or payload reading.
use super::*;
use polar_cas::shared::CompletePackIndex;
use polar_cas_local::{shared_index::SharedPackStore, LocalCas};
use std::os::unix::fs::PermissionsExt;
fn publish(store: &SharedPackStore, start: usize, count: usize, stop: &AtomicBool) {
    let mut body = format!("synthetic indexed pack {start} {count}").into_bytes();
    body.resize(4096, 0);
    let pack = ContentRef {
        id: ObjectId::for_bytes(&body),
        size_bytes: body.len() as u64,
    };
    let members = (start..start + count)
        .map(|i| ContentRef {
            id: id(i),
            size_bytes: (i + 100) as u64,
        })
        .collect();
    let admission = CompletePackIndex::from_trusted_adapter(pack.clone(), members).unwrap();
    let pending = store.prepare(&admission, stop).unwrap();
    let path = store.root().join(pack.id.content_key());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
    pending.publish().unwrap();
}
pub fn run(root: &std::path::Path) {
    let stop = AtomicBool::new(false);
    let store = SharedPackStore::open(LocalCas::open_shared(root.join("growth")).unwrap()).unwrap();
    for i in 0..64 {
        publish(&store, i * 16384, 16384, &stop);
    }
    loop {
        let receipt = store.aggregate(&stop).unwrap();
        if receipt.added_packs == 0 {
            break;
        }
    }
    assert_eq!(store.search_snapshot().unwrap().covered_packs(), 64);
    for i in 0..24 {
        let start = 1048576 + i * 8192;
        publish(&store, start, 8192, &stop);
        let now = Instant::now();
        let receipt = store.aggregate(&stop).unwrap();
        let ns = now.elapsed().as_nanos();
        assert!(
            receipt.rows_written < 1048576,
            "unrelated million-row run was rewritten"
        );
        assert!(store
            .search_snapshot()
            .unwrap()
            .locate(&id(start))
            .unwrap()
            .is_some());
        println!(
            "{}",
            json!({"kind":"growth","step":i,"base_rows":1048576,"added_rows":8192,"total_rows":start+8192,"aggregate_ns":ns,"receipt":receipt})
        );
    }
    store.audit().unwrap();
}
