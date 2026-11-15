use std::net::IpAddr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportEndpointHintProfile<'a> {
    pub backend: &'a str,
    pub protocol: &'a str,
    pub auth: &'a str,
}

pub trait TransportEndpointHintPolicy {
    fn backend_name(&self) -> &'static str;

    fn accepts_peer_endpoint_hint(
        &self,
        profile: TransportEndpointHintProfile<'_>,
        endpoints: &[String],
    ) -> bool;

    fn normalize_local_endpoint_hint(
        &self,
        profile: TransportEndpointHintProfile<'_>,
        endpoint: &str,
    ) -> String;
}

pub fn peer_endpoint_is_advertisable(endpoint: &str) -> bool {
    let endpoint = endpoint.trim();
    let (scheme, addr) = endpoint
        .split_once('/')
        .map_or(("", endpoint), |(scheme, addr)| (scheme, addr));
    if !matches!(scheme, "" | "tcp" | "tls" | "ws" | "wss") {
        return true;
    }
    let Some(host) = endpoint_host(addr) else {
        return true;
    };
    if host.eq_ignore_ascii_case("localhost") {
        return false;
    }
    host.parse::<IpAddr>()
        .map(|ip| !(ip.is_loopback() || ip.is_unspecified()))
        .unwrap_or(true)
}

fn endpoint_host(addr: &str) -> Option<&str> {
    if let Some(rest) = addr.strip_prefix('[') {
        return rest.split_once(']').map(|(host, _)| host);
    }
    let host = addr.split(':').next().unwrap_or(addr);
    (!host.is_empty()).then_some(host)
}


