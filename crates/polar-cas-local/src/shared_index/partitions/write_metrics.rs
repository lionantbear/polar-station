//! Development index phase accounting, disabled until a benchmark enables it.
use std::{cell::RefCell, collections::BTreeMap, time::Duration};
#[derive(Default, Clone, serde::Serialize)]
pub struct Stage {
    pub calls: u64,
    pub nanos: u128,
    pub bytes: u64,
    pub records: u64,
}
thread_local! {
    static STAGES: RefCell<Option<BTreeMap<&'static str, Stage>>> = const { RefCell::new(None) };
}
pub fn start() {
    STAGES.with(|s| *s.borrow_mut() = Some(BTreeMap::new()));
}
pub fn take() -> BTreeMap<&'static str, Stage> {
    STAGES.with(|s| s.borrow_mut().take().unwrap_or_default())
}
pub(crate) fn record(name: &'static str, elapsed: Duration, bytes: u64, records: u64) {
    STAGES.with(|s| {
        if let Some(stages) = s.borrow_mut().as_mut() {
            let value = stages.entry(name).or_default();
            value.calls += 1;
            value.nanos += elapsed.as_nanos();
            value.bytes += bytes;
            value.records += records;
        }
    });
}
