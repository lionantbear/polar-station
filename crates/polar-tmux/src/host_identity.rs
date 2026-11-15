use std::{
    fmt,
    sync::{Mutex, OnceLock},
};

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

const SERVICE: &str = "com.polarealm.PolarStation";
const LEGACY_SERVICES: &[&str] = &["io.polarealm.PolarStation", "io.polarealm.PolarTmux"];
const ACCOUNT: &str = "host-identity-v1";
static IDENTITY_CREATION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub struct HostIdentity {
    host_id: String,
    root_secret: Zeroizing<[u8; 32]>,
}

impl fmt::Debug for HostIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostIdentity")
            .field("host_id", &self.host_id)
            .field("root_secret", &"<redacted>")
            .finish()
    }
}

impl HostIdentity {
    pub fn load_or_create(store: &impl CredentialStore) -> Result<Self> {
        let _creation_guard = IDENTITY_CREATION_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(encoded) = store.load(SERVICE, ACCOUNT)? {
            let encoded = Zeroizing::new(encoded);
            return Self::decode(&encoded);
        }
        for legacy_service in LEGACY_SERVICES {
            let Some(encoded) = store.load(legacy_service, ACCOUNT)? else {
                continue;
            };
            let encoded = Zeroizing::new(encoded);
            let identity = Self::decode(&encoded)?;
            store
                .save(SERVICE, ACCOUNT, &encoded)
                .context("failed to migrate Polar Station Host identity")?;
            return Ok(identity);
        }

        let mut secret = [0_u8; 32];
        OsRng.fill_bytes(&mut secret);
        let identity = Self {
            host_id: random_uuid_v4(),
            root_secret: Zeroizing::new(secret),
        };
        let encoded = identity.encode()?;
        store.save(SERVICE, ACCOUNT, &encoded)?;
        Ok(identity)
    }

    pub fn load_platform() -> Result<Self> {
        #[cfg(unix)]
        let _cross_process_guard = crate::local_ipc::acquire_identity_creation_lock()?;
        Self::load_or_create(&PlatformCredentialStore)
    }

    /// Keyring's portable Linux Secret Service adapter is synchronous and
    /// internally owns a small async runtime. Keep it off the Host's Tokio
    /// runtime thread so the adapter cannot nest runtimes and panic.
    pub fn load_platform_isolated() -> Result<Self> {
        std::thread::spawn(Self::load_platform)
            .join()
            .map_err(|_| anyhow::anyhow!("Host credential-store thread panicked"))?
    }

    pub fn host_id(&self) -> &str {
        &self.host_id
    }

    pub fn root_secret(&self) -> &[u8; 32] {
        &self.root_secret
    }

    /// Build the local-only Companion pairing URI without exposing the root
    /// secret in logs or debug output.
    pub fn pairing_code(&self) -> Zeroizing<String> {
        let encoded_key = Zeroizing::new(STANDARD.encode(self.root_secret()));
        let mut escaped_key = Zeroizing::new(String::with_capacity(encoded_key.len() * 3));
        for byte in encoded_key.bytes() {
            match byte {
                b'+' => escaped_key.push_str("%2B"),
                b'/' => escaped_key.push_str("%2F"),
                b'=' => escaped_key.push_str("%3D"),
                _ => escaped_key.push(char::from(byte)),
            }
        }
        Zeroizing::new(format!(
            "polar-tmux://pair?v=1&host_id={}&key={}",
            self.host_id(),
            escaped_key.as_str()
        ))
    }


    fn encode(&self) -> Result<Zeroizing<String>> {
        let root_secret = Zeroizing::new(STANDARD.encode(&self.root_secret[..]));
        serde_json::to_string(&StoredIdentity {
            version: 1,
            host_id: &self.host_id,
            root_secret: &root_secret,
        })
        .map(Zeroizing::new)
        .context("failed to encode Polar Station Host identity")
    }

    fn decode(encoded: &str) -> Result<Self> {
        let stored: StoredIdentity<'_> =
            serde_json::from_str(encoded).context("stored Host identity is invalid JSON")?;
        if stored.version != 1 || !looks_like_uuid(stored.host_id) {
            bail!("stored Host identity metadata is invalid");
        }
        let decoded = Zeroizing::new(
            STANDARD
                .decode(stored.root_secret)
                .context("stored Host root secret is invalid base64")?,
        );
        if decoded.len() != 32 {
            bail!("stored Host root secret must contain 32 bytes");
        }
        let mut root_secret = Zeroizing::new([0_u8; 32]);
        root_secret.copy_from_slice(&decoded);
        Ok(Self {
            host_id: stored.host_id.to_string(),
            root_secret,
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredIdentity<'a> {
    version: u32,
    #[serde(borrow)]
    host_id: &'a str,
    #[serde(borrow)]
    root_secret: &'a str,
}

pub trait CredentialStore {
    fn load(&self, service: &str, account: &str) -> Result<Option<String>>;
    fn save(&self, service: &str, account: &str, value: &str) -> Result<()>;
}

pub(crate) fn load_peer_key(
    store: &(dyn CredentialStore + Send + Sync),
    source_host_id: &str,
    scope: polar_tmux_protocol::HostPeerScopeV1,
) -> Result<Zeroizing<[u8; 32]>> {
    let account = scope.credential_account(source_host_id)?;
    let encoded = Zeroizing::new(
        store
            .load("com.polarealm.PolarStation.peers", &account)
            .map_err(|_| anyhow::anyhow!("peer credential unavailable"))?
            .ok_or_else(|| anyhow::anyhow!("peer is not paired"))?,
    );
    let bytes = Zeroizing::new(
        STANDARD
            .decode(encoded.as_bytes())
            .map_err(|_| anyhow::anyhow!("invalid peer credential"))?,
    );
    anyhow::ensure!(bytes.len() == 32, "invalid peer credential length");
    let mut key = Zeroizing::new([0; 32]);
    key.copy_from_slice(&bytes);
    Ok(key)
}

pub(crate) struct PlatformCredentialStore;

impl CredentialStore for PlatformCredentialStore {
    fn load(&self, service: &str, account: &str) -> Result<Option<String>> {
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        {
            let entry =
                keyring::Entry::new(service, account).context("failed to open credential store")?;
            match entry.get_password() {
                Ok(value) => Ok(Some(value)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(error) => Err(error).context("failed to read Host identity"),
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            let _ = (service, account);
            bail!("this platform has no configured secure credential store")
        }
    }

    fn save(&self, service: &str, account: &str, value: &str) -> Result<()> {
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        {
            let entry =
                keyring::Entry::new(service, account).context("failed to open credential store")?;
            entry
                .set_password(value)
                .context("failed to persist Host identity")
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            let _ = (service, account, value);
            bail!("this platform has no configured secure credential store")
        }
    }
}

/// Fresh opaque request/registration identity shared by Host and CLI adapters.
pub fn random_uuid_v4() -> String {
    let mut bytes = [0_u8; 16];
    OsRng.fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

fn looks_like_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}


