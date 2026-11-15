//! Application runtime policy. Platform adapters never choose Host shutdown.
use std::{future::Future, time::Duration};

pub fn run_standalone_runtime<F>(future: F) -> anyhow::Result<()>
where
    F: Future<Output = anyhow::Result<()>>,
{
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let result = runtime.block_on(future);
    // Host has already drained domain-owned children, or returned a cleanup
    // error. A platform consent call can still wait on a user indefinitely;
    // runtime disposal is not evidence that domain cleanup succeeded.
    runtime.shutdown_timeout(Duration::from_secs(1));
    result
}
