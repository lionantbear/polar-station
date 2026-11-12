//! One fixed secret/owner resolver per CPU use session. No global cache,
//! unbounded map, public raw-key RPC, or automatic fallback. A single expiry
//! task owns no key bytes and wipes idle cached material at its deadline.
use super::{CredentialResolver, SecretId, SecretLease};
use crate::protection::{
    ProtectionCapabilities, SecretCompute, SecretProtection, SecretRetention, SecretStorage,
};
use anyhow::{ensure, Result};
use std::{
    sync::{Arc, Mutex, MutexGuard, Weak},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Default)]
pub struct SecretUseTiming {
    pub cache_hit: bool,
    /// Resolver I/O + decrypt; excludes the separate metadata freshness check.
    pub resolve_ns: u128,
    pub validate_ns: u128,
    pub operation_ns: u128,
    /// Synchronous drop/zeroization or cache/timer installation, not idle expiry.
    pub retention_ns: u128,
}

pub struct SecretUseSession {
    resolver: Arc<dyn CredentialResolver>,
    id: SecretId,
    version: u64,
    policy: SecretProtection,
    max_bytes: usize,
    cached: Arc<Mutex<Option<CacheEntry>>>,
    expiry: Option<tokio::task::JoinHandle<()>>,
    invalidation: Option<tokio::task::JoinHandle<()>>,
    invalidation_epoch: Option<tokio::sync::watch::Receiver<u64>>,
    generation: u64,
    closed: bool,
    timing: Option<SecretUseTiming>,
}

impl SecretUseSession {
    pub fn new(
        resolver: Arc<dyn CredentialResolver>,
        id: SecretId,
        version: u64,
        policy: SecretProtection,
        max_bytes: usize,
    ) -> Result<Self> {
        ensure!(
            version > 0 && max_bytes > 0 && max_bytes <= 65_536,
            "invalid secret use limits"
        );
        ensure!(
            policy.compute == SecretCompute::Host
                && matches!(
                    policy.storage,
                    SecretStorage::Vault | SecretStorage::HostMemory
                ),
            "CPU session cannot hold device or hardware secrets"
        );
        policy
            .validate(ProtectionCapabilities {
                host: true,
                device_vault: true,
                ..ProtectionCapabilities::default()
            })
            .map_err(|_| anyhow::anyhow!("invalid secret protection policy"))?;
        let cached = Arc::new(Mutex::new(None));
        let mut invalidation_epoch = None;
        let invalidation = if matches!(policy.retention, SecretRetention::Session { .. }) {
            let mut changes = resolver
                .subscribe_invalidation()
                .ok_or_else(|| anyhow::anyhow!("session cache requires invalidation events"))?;
            invalidation_epoch = Some(changes.clone());
            let weak = Arc::downgrade(&cached);
            let runtime = tokio::runtime::Handle::try_current()?;
            Some(runtime.spawn(async move {
                while changes.changed().await.is_ok() {
                    let Some(cache) = weak.upgrade() else {
                        return;
                    };
                    lock(&cache).take();
                }
                if let Some(cache) = weak.upgrade() {
                    lock(&cache).take();
                }
            }))
        } else {
            None
        };
        Ok(Self {
            resolver,
            id,
            version,
            policy,
            max_bytes,
            cached,
            invalidation,
            invalidation_epoch,
            expiry: None,
            generation: 0,
            closed: false,
            timing: None,
        })
    }

    /// Lock, transport loss, session end and explicit revocation all use this
    /// permanent invalidation. A new authorization must construct a new owner.
    pub fn close(&mut self) {
        self.clear_cache();
        if let Some(task) = self.invalidation.take() {
            task.abort();
        }
        self.closed = true;
    }

    pub fn fixed_secret(&self) -> (SecretId, u64) {
        (self.id, self.version)
    }
    pub fn cache_resident(&self) -> bool {
        lock(&self.cached).is_some()
    }
    pub fn last_timing(&self) -> Option<SecretUseTiming> {
        self.timing
    }

    fn clear_cache(&mut self) {
        lock(&self.cached).take();
        if let Some(task) = self.expiry.take() {
            task.abort();
        }
    }

    /// Only the authorized C1 executor receives the bytes. The closure runs
    /// synchronously: async cancellation drops any in-flight Zeroizing lease.
    pub async fn with_secret<T>(
        &mut self,
        operation: impl FnOnce(&[u8]) -> Result<T>,
    ) -> Result<T> {
        self.timing = None;
        let mut timing = SecretUseTiming::default();
        let epoch = self.invalidation_epoch.as_ref().map(|r| *r.borrow());
        ensure!(!self.closed, "secret use session is closed");
        // Take before awaiting: cancellation must not leave a cached copy in
        // the session after its freshness check was abandoned.
        if let Some(task) = self.expiry.take() {
            task.abort();
        }
        let previous = lock(&self.cached).take();
        let lease = match previous {
            Some(CacheEntry {
                lease, deadline, ..
            }) if Instant::now() < deadline => {
                let started = Instant::now();
                let current = self.resolver.current_version(self.id).await?;
                timing.validate_ns += started.elapsed().as_nanos();
                ensure!(current == self.version, "credential version changed");
                timing.cache_hit = true;
                (lease, Some(deadline))
            }
            other => {
                drop(other);
                let started = Instant::now();
                let lease = self.resolver.resolve_for_use(self.id).await?;
                timing.resolve_ns += started.elapsed().as_nanos();
                ensure!(
                    lease.id() == self.id && lease.version() == self.version,
                    "credential version changed"
                );
                if matches!(self.policy.retention, SecretRetention::Session { .. }) {
                    let started = Instant::now();
                    let current = self.resolver.current_version(self.id).await?;
                    timing.validate_ns += started.elapsed().as_nanos();
                    ensure!(current == self.version, "credential version changed");
                }
                let deadline = match self.policy.retention {
                    SecretRetention::Session { ttl_ms } => {
                        Some(Instant::now() + Duration::from_millis(u64::from(ttl_ms)))
                    }
                    _ => None,
                };
                (lease, deadline)
            }
        };
        ensure!(
            lease.0.expose_bytes().len() <= self.max_bytes,
            "credential exceeds session capacity"
        );
        ensure!(
            epoch == self.invalidation_epoch.as_ref().map(|r| *r.borrow()),
            "credential invalidated during resolution"
        );
        ensure!(
            lease.1.is_none_or(|deadline| Instant::now() < deadline),
            "credential lease expired during resolution"
        );
        let started = Instant::now();
        let result = operation(lease.0.expose_bytes());
        timing.operation_ns = started.elapsed().as_nanos();
        let started = Instant::now();
        let (lease, deadline) = lease;
        let mut lease = Some(lease);
        if result.is_ok() && epoch == self.invalidation_epoch.as_ref().map(|r| *r.borrow()) {
            if let Some(deadline) = deadline {
                if Instant::now() < deadline {
                    // A runtime is required for a bounded cache. Never silently
                    // keep a key when no executor can enforce idle expiry.
                    let runtime = tokio::runtime::Handle::try_current()?;
                    self.generation = self.generation.wrapping_add(1);
                    let generation = self.generation;
                    *lock(&self.cached) = Some(CacheEntry {
                        lease: lease.take().expect("one lease"),
                        deadline,
                        generation,
                    });
                    let guard = ExpiryGuard {
                        cache: Arc::downgrade(&self.cached),
                        generation,
                    };
                    self.expiry = Some(runtime.spawn(async move {
                        let _guard = guard;
                        tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
                        // Drop clears even if this task is cancelled before its first poll.
                    }));
                }
            }
        }
        // Errors, expiry and operation retention drop and zeroize the lease.
        drop(lease);
        timing.retention_ns = started.elapsed().as_nanos();
        self.timing = Some(timing);
        if epoch != self.invalidation_epoch.as_ref().map(|r| *r.borrow()) {
            self.clear_cache();
            anyhow::bail!("credential invalidated during operation");
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            self.clear_cache();
            anyhow::bail!("credential lease expired during operation");
        }
        result
    }
}

impl Drop for SecretUseSession {
    fn drop(&mut self) {
        self.clear_cache();
        if let Some(task) = self.invalidation.take() {
            task.abort();
        }
    }
}

struct CacheEntry {
    lease: SecretLease,
    deadline: Instant,
    generation: u64,
}
fn lock(cache: &Mutex<Option<CacheEntry>>) -> MutexGuard<'_, Option<CacheEntry>> {
    // No user code runs under this lock. A poisoned guard must still wipe keys.
    cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
struct ExpiryGuard {
    cache: Weak<Mutex<Option<CacheEntry>>>,
    generation: u64,
}
impl Drop for ExpiryGuard {
    fn drop(&mut self) {
        if let Some(cache) = self.cache.upgrade() {
            let mut entry = lock(&cache);
            if entry
                .as_ref()
                .is_some_and(|e| e.generation == self.generation)
            {
                entry.take();
            }
        }
    }
}


