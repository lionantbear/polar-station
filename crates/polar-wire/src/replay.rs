//! Historical stream replay chunk bodies were retired in the v6 live registry.
//!
//! Replay now operates over exact raw packet bytes from the packet journal and
//! uses packet stream window reports for repair requests.
