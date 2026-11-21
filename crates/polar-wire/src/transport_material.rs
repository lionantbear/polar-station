//! Bounded canonical bodies for direct transport material maintenance (37/38).
use anyhow::{ensure, Context, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_bytes::ByteBuf;
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportQuery(pub ByteBuf, pub String, pub String);

/// Grant ID, sender AP, receiver AP, revision, query digest, credentials, planes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportSnapshot(
    pub ByteBuf,
    pub String,
    pub String,
    pub u64,
    pub Option<ByteBuf>,
    pub Vec<TransportCredential>,
    pub Vec<AdvertisedTransport>,
);

/// ID 1–4, material kind (1 = peer CA DER), public bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportCredential(pub u16, pub u8, pub ByteBuf);

/// Stable reference, backend, protocol, auth, credential IDs, locators.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdvertisedTransport(
    pub ByteBuf,
    pub u8,
    pub u8,
    pub u8,
    pub Vec<u16>,
    pub Vec<TransportLocator>,
);

/// Host kind (1 = IPv4, 2 = IPv6, 3 = DNS), raw host bytes, port.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportLocator(pub u8, pub ByteBuf, pub u16);

pub const ZENOH: u8 = 1;
pub const TCP_TLS: u8 = 1;
pub const WEBSOCKET_TLS: u8 = 2;
pub const MUTUAL_TLS: u8 = 1;
pub const PEER_CA: u8 = 1;

impl TransportLocator {
    pub fn parse(locator: &str) -> Result<(u8, Self)> {
        let (protocol, authority) = locator
            .split_once('/')
            .context("missing transport protocol")?;
        let protocol = match protocol {
            "tls" => TCP_TLS,
            "wss" => WEBSOCKET_TLS,
            _ => anyhow::bail!("unsupported transport protocol"),
        };
        let (host, port) = authority
            .rsplit_once(':')
            .context("missing transport port")?;
        let port = port.parse()?;
        let locator = match host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
        {
            Ok(std::net::IpAddr::V4(ip)) => Self(1, ip.octets().to_vec().into(), port),
            Ok(std::net::IpAddr::V6(ip)) => Self(2, ip.octets().to_vec().into(), port),
            Err(_) => Self(3, host.to_ascii_lowercase().into_bytes().into(), port),
        };
        locator.to_endpoint(protocol)?;
        Ok((protocol, locator))
    }

    pub fn to_endpoint(&self, protocol: u8) -> Result<String> {
        let host = match self.0 {
            1 => {
                let octets: [u8; 4] = self.1.as_ref().try_into()?;
                std::net::Ipv4Addr::from(octets).to_string()
            }
            2 => {
                let octets: [u8; 16] = self.1.as_ref().try_into()?;
                format!("[{}]", std::net::Ipv6Addr::from(octets))
            }
            3 => {
                let dns = std::str::from_utf8(self.1.as_ref())?;
                ensure!(
                    dns.len() <= 253
                        && dns.split('.').all(|label| {
                            !label.is_empty()
                                && label.len() <= 63
                                && !label.starts_with('-')
                                && !label.ends_with('-')
                                && label.bytes().all(|b| {
                                    b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'
                                })
                        }),
                    "invalid advertised DNS host"
                );
                dns.to_string()
            }
            _ => anyhow::bail!("unsupported host kind"),
        };
        let protocol = match protocol {
            TCP_TLS => "tls",
            WEBSOCKET_TLS => "wss",
            _ => anyhow::bail!("unsupported transport protocol"),
        };
        ensure!(self.2 != 0, "zero advertised port");
        Ok(format!("{protocol}/{host}:{}", self.2))
    }
}

impl TransportQuery {
    pub fn encode(&self) -> Result<Vec<u8>> {
        validate_scope(&self.0, &self.1, &self.2)?;
        encode(self)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let result: Self = decode(bytes)?;
        validate_scope(&result.0, &result.1, &result.2)?;
        Ok(result)
    }
}

impl TransportSnapshot {
    pub fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        encode(self)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let result: Self = decode(bytes)?;
        result.validate()?;
        Ok(result)
    }
    /// Stable across retransmission and Query replies.
    pub fn material_digest(&self) -> Result<[u8; 32]> {
        Ok(crate::sha256_bytes(&encode(&(&self.5, &self.6))?))
    }
    fn validate(&self) -> Result<()> {
        validate_scope(&self.0, &self.1, &self.2)?;
        ensure!(
            self.3 != 0 && self.4.as_ref().is_none_or(|v| v.len() == 32),
            "invalid material revision or Query digest"
        );
        ensure!(
            self.5.len() <= 4 && self.6.len() <= 4,
            "transport snapshot entry limit"
        );
        let mut credentials = BTreeSet::new();
        for TransportCredential(id, kind, value) in &self.5 {
            ensure!(
                (1..=4).contains(id)
                    && *kind == PEER_CA
                    && !value.is_empty()
                    && value.len() <= 2048
                    && credentials.insert(id),
                "invalid transport credential"
            );
        }
        let mut references = BTreeSet::new();
        for AdvertisedTransport(reference, backend, protocol, auth, ids, locators) in &self.6 {
            ensure!(
                reference.len() == 16
                    && references.insert(reference)
                    && *backend == ZENOH
                    && *auth == MUTUAL_TLS
                    && ids.len() == 1
                    && credentials.contains(&ids[0])
                    && !locators.is_empty()
                    && locators.len() <= 4,
                "invalid advertised transport"
            );
            let mut seen = BTreeSet::new();
            for locator in locators {
                ensure!(
                    seen.insert(locator.to_endpoint(*protocol)?),
                    "duplicate transport locator"
                );
            }
        }
        Ok(())
    }
}

fn validate_scope(grant: &[u8], sender: &str, recipient: &str) -> Result<()> {
    ensure!(
        grant.len() == 16 && grant.iter().any(|b| *b != 0),
        "invalid grant ID"
    );
    for plane in [sender, recipient] {
        ensure!(
            !plane.is_empty()
                && plane.len() <= 96
                && plane
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b)),
            "invalid access plane"
        );
    }
    Ok(())
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let bytes = serde_cbor::to_vec(value)?;
    ensure!(
        bytes.len() <= crate::MAX_ADMISSION_BODY_BYTES,
        "transport material exceeds size limit"
    );
    Ok(bytes)
}
fn decode<T: DeserializeOwned + Serialize>(bytes: &[u8]) -> Result<T> {
    ensure!(
        bytes.len() <= crate::MAX_ADMISSION_BODY_BYTES,
        "transport material exceeds size limit"
    );
    let value = serde_cbor::from_slice(bytes)?;
    ensure!(encode(&value)? == bytes, "noncanonical transport material");
    Ok(value)
}


