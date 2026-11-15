//! Route policy, resolution and platform network notifications; no pane ownership.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SshRouteKind {
    Opaque,
    Lan,
    Wan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SshRoute {
    Opaque,
    Lan {
        address: Ipv4Addr,
        port: u16,
        host_key_alias: String,
    },
    Wan {
        address: Ipv4Addr,
        port: u16,
        host_key_alias: String,
    },
}

impl SshRoute {
    pub(super) fn kind(&self) -> SshRouteKind {
        match self {
            Self::Opaque => SshRouteKind::Opaque,
            Self::Lan { .. } => SshRouteKind::Lan,
            Self::Wan { .. } => SshRouteKind::Wan,
        }
    }

    pub(super) fn proxy_endpoint(&self) -> Option<(String, u16)> {
        match self {
            Self::Opaque => None,
            Self::Lan { address, port, .. } | Self::Wan { address, port, .. } => {
                Some((address.to_string(), *port))
            }
        }
    }

    pub(super) fn host_key_alias(&self) -> Option<&str> {
        match self {
            Self::Opaque => None,
            Self::Lan { host_key_alias, .. } | Self::Wan { host_key_alias, .. } => {
                Some(host_key_alias)
            }
        }
    }

    pub(super) fn socket_tag(&self) -> &'static str {
        match self {
            Self::Opaque => "o",
            Self::Lan { .. } => "l",
            Self::Wan { .. } => "w",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DohProvider {
    Google,
    Cloudflare,
}

pub(super) fn default_doh_providers() -> Vec<DohProvider> {
    vec![DohProvider::Google, DohProvider::Cloudflare]
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SshRouteProfile {
    pub(crate) lan_address: Ipv4Addr,
    #[serde(default = "default_ssh_port")]
    pub(crate) lan_port: u16,
    pub(crate) wan_host: String,
    pub(crate) wan_port: u16,
    #[serde(default = "default_doh_providers")]
    pub(crate) doh_providers: Vec<DohProvider>,
}

const fn default_ssh_port() -> u16 {
    22
}

impl SshRouteProfile {
    pub(crate) fn validate(&self) -> bool {
        self.lan_port != 0
            && self.wan_port != 0
            && !self.doh_providers.is_empty()
            && self.doh_providers.len() <= 2
            && valid_dns_name(&self.wan_host)
    }

    
}

pub(super) fn openssh_known_hosts_alias(host: &str, port: u16) -> String {
    if port == default_ssh_port() {
        host.to_owned()
    } else {
        format!("[{host}]:{port}")
    }
}

pub(super) fn lan_route(profile: &SshRouteProfile) -> SshRoute {
    SshRoute::Lan {
        address: profile.lan_address,
        port: profile.lan_port,
        // The standard known_hosts identity for a direct address is the LAN
        // IP (or [IP]:port for a non-default port). Reusing the WAN DNS alias
        // at port 22 would require a different, usually absent trust entry.
        host_key_alias: openssh_known_hosts_alias(
            &profile.lan_address.to_string(),
            profile.lan_port,
        ),
    }
}

pub(super) fn wan_route(profile: &SshRouteProfile, address: Ipv4Addr) -> SshRoute {
    SshRoute::Wan {
        address,
        port: profile.wan_port,
        host_key_alias: openssh_known_hosts_alias(&profile.wan_host, profile.wan_port),
    }
}

pub(super) fn valid_dns_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LocalIpv4Subnet {
    pub(super) interface_name: String,
    pub(super) address: Ipv4Addr,
    pub(super) netmask: Ipv4Addr,
    pub(super) physical: bool,
}

impl LocalIpv4Subnet {
    

    pub(super) fn contains(&self, candidate: Ipv4Addr) -> bool {
        if !self.physical {
            return false;
        }
        let mask = u32::from(self.netmask);
        u32::from(self.address) & mask == u32::from(candidate) & mask
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct LocalNetworkSnapshot {
    pub(super) subnets: Vec<LocalIpv4Subnet>,
}

impl LocalNetworkSnapshot {
    pub(super) fn new(subnets: impl IntoIterator<Item = LocalIpv4Subnet>) -> Self {
        Self {
            subnets: subnets.into_iter().collect(),
        }
    }

    pub(super) fn authorizes_lan_address(&self, address: Ipv4Addr) -> bool {
        self.subnets.iter().any(|subnet| subnet.contains(address))
    }

    
}

#[derive(Debug, Clone)]
pub(super) struct ResolvedWanRoute {
    pub(super) addresses: Vec<Ipv4Addr>,
    pub(super) ttl: Duration,
}

pub(super) trait NetworkRouteBackend: Send + Sync {
    fn local_network(&self) -> io::Result<LocalNetworkSnapshot>;
    fn lan_is_direct(&self, address: Ipv4Addr) -> io::Result<bool> {
        self.local_network()
            .map(|snapshot| snapshot.authorizes_lan_address(address))
    }
    fn resolve_wan<'a>(&'a self, profile: &'a SshRouteProfile) -> ResolveFuture<'a>;
}

#[derive(Debug, Default)]
pub(super) struct SystemNetworkRouteBackend;

impl NetworkRouteBackend for SystemNetworkRouteBackend {
    fn local_network(&self) -> io::Result<LocalNetworkSnapshot> {
        system_local_network_snapshot()
    }

    fn lan_is_direct(&self, address: Ipv4Addr) -> io::Result<bool> {
        let snapshot = system_local_network_snapshot()?;
        system_route_is_physical_direct(&snapshot, address)
    }

    fn resolve_wan<'a>(&'a self, profile: &'a SshRouteProfile) -> ResolveFuture<'a> {
        Box::pin(resolve_wan_sequential(
            &profile.doh_providers,
            &profile.wan_host,
            |provider, host| async move { resolve_doh_provider(provider, &host).await },
        ))
    }
}

pub(super) async fn resolve_wan_sequential<F, Fut>(
    providers: &[DohProvider],
    host: &str,
    mut resolve: F,
) -> io::Result<ResolvedWanRoute>
where
    F: FnMut(DohProvider, String) -> Fut,
    Fut: Future<Output = io::Result<ResolvedWanRoute>>,
{
    let mut last_error = io::Error::other("no approved DoH provider returned an address");
    for provider in providers {
        match resolve(*provider, host.to_owned()).await {
            Ok(resolution) => return Ok(resolution),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

/// Subscribe to kernel route/address changes without generating network
/// traffic. The notification is deliberately content-free: the shared route
/// manager re-reads local interfaces and decides whether a LAN candidate is
/// authorized.
#[cfg(any(target_os = "macos", test))]
pub(super) fn darwin_route_buffer_contains_change(mut buffer: &[u8]) -> bool {
    const DARWIN_ROUTE_HEADER_PREFIX_LEN: usize = 4;
    const DARWIN_RTM_VERSION: u8 = 5;

    while !buffer.is_empty() {
        if buffer.len() < DARWIN_ROUTE_HEADER_PREFIX_LEN {
            return false;
        }
        let message_len = usize::from(u16::from_ne_bytes([buffer[0], buffer[1]]));
        if message_len < DARWIN_ROUTE_HEADER_PREFIX_LEN || message_len > buffer.len() {
            return false;
        }
        let message_version = buffer[2];
        let message_type = buffer[3];
        if message_version == DARWIN_RTM_VERSION
            && matches!(
                message_type,
                0x01 // RTM_ADD
                    | 0x02 // RTM_DELETE
                    | 0x03 // RTM_CHANGE
                    | 0x0c // RTM_NEWADDR
                    | 0x0d // RTM_DELADDR
                    | 0x0e // RTM_IFINFO
                    | 0x0f // RTM_NEWMADDR
                    | 0x10 // RTM_DELMADDR
                    | 0x12 // RTM_IFINFO2
                    | 0x13 // RTM_NEWMADDR2
            )
        {
            return true;
        }
        buffer = &buffer[message_len..];
    }
    false
}

pub(super) fn spawn_platform_network_monitor(manager: Weak<ManagerInner>) {
    if let Some(inner) = manager.upgrade() {
        record_recovery_trace(&inner, "network_monitor_started", None, None, None);
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    std::thread::Builder::new()
        .name("polar-station-network-path".to_owned())
        .spawn(move || platform_network_monitor(manager))
        .expect("spawn Polar Station network path monitor");

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let _ = manager;
}

#[cfg(target_os = "macos")]
pub(super) fn platform_network_monitor(manager: Weak<ManagerInner>) {
    // SAFETY: PF_ROUTE returns a process-owned descriptor. No borrowed pointer
    // crosses this call and the descriptor is closed before the thread exits.
    let descriptor = unsafe { libc::socket(libc::PF_ROUTE, libc::SOCK_RAW, 0) };
    if descriptor < 0 {
        if let Some(inner) = manager.upgrade() {
            record_recovery_trace(&inner, "network_monitor_failed", None, None, None);
        }
        return;
    }
    monitor_descriptor(descriptor, manager);
}

#[cfg(target_os = "linux")]
pub(super) fn platform_network_monitor(manager: Weak<ManagerInner>) {
    // SAFETY: AF_NETLINK returns a process-owned descriptor. The zeroed
    // sockaddr_nl is filled with the documented kernel multicast groups before
    // bind, and its pointer remains valid for the duration of the call.
    let descriptor = unsafe { libc::socket(libc::AF_NETLINK, libc::SOCK_RAW, libc::NETLINK_ROUTE) };
    if descriptor < 0 {
        if let Some(inner) = manager.upgrade() {
            record_recovery_trace(&inner, "network_monitor_failed", None, None, None);
        }
        return;
    }
    let mut address: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    address.nl_family = libc::AF_NETLINK as u16;
    address.nl_groups =
        (libc::RTMGRP_LINK | libc::RTMGRP_IPV4_IFADDR | libc::RTMGRP_IPV4_ROUTE) as u32;
    // SAFETY: descriptor and address are valid, and the supplied length is the
    // exact size of sockaddr_nl.
    let bound = unsafe {
        libc::bind(
            descriptor,
            (&raw const address).cast(),
            std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
        )
    };
    if bound != 0 {
        // SAFETY: descriptor is owned by this thread and has not been closed.
        unsafe { libc::close(descriptor) };
        if let Some(inner) = manager.upgrade() {
            record_recovery_trace(&inner, "network_monitor_failed", None, None, None);
        }
        return;
    }
    monitor_descriptor(descriptor, manager);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(super) fn monitor_descriptor(descriptor: libc::c_int, manager: Weak<ManagerInner>) {
    let receive_timeout = libc::timeval {
        tv_sec: 30,
        tv_usec: 0,
    };
    // SAFETY: descriptor is owned by this thread and receive_timeout remains
    // live for the complete setsockopt call.
    unsafe {
        libc::setsockopt(
            descriptor,
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            (&raw const receive_timeout).cast(),
            std::mem::size_of::<libc::timeval>() as libc::socklen_t,
        );
    }
    let mut buffer = [0_u8; 8192];
    loop {
        if manager.strong_count() == 0 {
            break;
        }
        // SAFETY: buffer is writable for its complete length and descriptor is
        // owned by this thread. recv does not retain the supplied pointer.
        let read = unsafe { libc::recv(descriptor, buffer.as_mut_ptr().cast(), buffer.len(), 0) };
        if read > 0 {
            #[cfg(target_os = "macos")]
            let contains_change = darwin_route_buffer_contains_change(&buffer[..read as usize]);
            #[cfg(target_os = "linux")]
            let contains_change = true;
            if !contains_change {
                continue;
            }
            let Some(manager) = manager.upgrade() else {
                break;
            };
            tracing::info!(target: "polar_station_operation", component="Network", event="network_event");
            record_recovery_trace(&manager, "network_event", None, None, None);
            let _ = manager.network_events.send(());
        }
    }
    // SAFETY: descriptor is owned by this thread and closed exactly once.
    unsafe { libc::close(descriptor) };
}

#[derive(Debug, Deserialize)]
pub(super) struct DohJsonResponse {
    #[serde(rename = "Status")]
    pub(super) status: i64,
    #[serde(rename = "Answer", default)]
    pub(super) answers: Vec<DohJsonAnswer>,
}

#[derive(Debug, Deserialize)]
pub(super) struct DohJsonAnswer {
    #[serde(rename = "type")]
    pub(super) record_type: u16,
    #[serde(rename = "TTL")]
    pub(super) ttl: u64,
    pub(super) data: String,
}

pub(super) async fn resolve_doh_provider(
    provider: DohProvider,
    host: &str,
) -> io::Result<ResolvedWanRoute> {
    let (base, pinned_provider) = match provider {
        DohProvider::Google => ("https://dns.google/resolve", "dns.google:443:8.8.8.8"),
        DohProvider::Cloudflare => (
            "https://cloudflare-dns.com/dns-query",
            "cloudflare-dns.com:443:1.1.1.1",
        ),
    };
    let output = tokio::time::timeout(
        Duration::from_secs(4),
        Command::new("/usr/bin/curl")
            .args([
                "-fsS",
                "--max-time",
                "3",
                "--noproxy",
                "*",
                "--resolve",
                pinned_provider,
                "-H",
                "Accept: application/dns-json",
            ])
            .arg(format!("{base}?name={host}&type=A"))
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output(),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "DoH provider timed out"))??;
    if !output.status.success() {
        return Err(io::Error::other("DoH provider failed"));
    }
    let response: DohJsonResponse = serde_json::from_slice(&output.stdout)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid DoH response"))?;
    if response.status != 0 {
        return Err(io::Error::other("DoH provider returned a DNS error"));
    }
    let mut addresses = Vec::new();
    let mut ttl = u64::MAX;
    for answer in response.answers {
        if answer.record_type == 1 {
            if let Ok(address) = answer.data.parse::<Ipv4Addr>() {
                if !addresses.contains(&address) {
                    addresses.push(address);
                }
                ttl = ttl.min(answer.ttl);
            }
        }
    }
    if addresses.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "DoH provider returned no IPv4 address",
        ));
    }
    Ok(ResolvedWanRoute {
        addresses,
        ttl: Duration::from_secs(ttl.clamp(30, 86_400)),
    })
}

#[cfg(unix)]
pub(super) fn system_local_network_snapshot() -> io::Result<LocalNetworkSnapshot> {
    let mut head = std::ptr::null_mut();
    // SAFETY: getifaddrs initializes a linked list owned by libc on success;
    // every sockaddr is read only after its family and pointers are checked.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut subnets = Vec::new();
    let mut cursor = head;
    while !cursor.is_null() {
        // SAFETY: cursor belongs to the live getifaddrs list.
        let entry = unsafe { &*cursor };
        if !entry.ifa_addr.is_null()
            && !entry.ifa_netmask.is_null()
            // SAFETY: both sockaddr pointers are non-null for this entry.
            && unsafe { (*entry.ifa_addr).sa_family as i32 } == libc::AF_INET
        {
            // SAFETY: AF_INET permits casting to sockaddr_in.
            let address = unsafe { *(entry.ifa_addr as *const libc::sockaddr_in) };
            // SAFETY: the netmask for an AF_INET entry is sockaddr_in.
            let netmask = unsafe { *(entry.ifa_netmask as *const libc::sockaddr_in) };
            // SAFETY: getifaddrs provides a NUL-terminated interface name.
            let name = unsafe { std::ffi::CStr::from_ptr(entry.ifa_name) }
                .to_string_lossy()
                .into_owned();
            let flags = entry.ifa_flags as i32;
            let physical = flags & libc::IFF_UP != 0
                && flags & libc::IFF_LOOPBACK == 0
                && flags & libc::IFF_POINTOPOINT == 0
                && flags & libc::IFF_BROADCAST != 0
                && physical_interface_name(&name);
            subnets.push(LocalIpv4Subnet {
                interface_name: name,
                address: Ipv4Addr::from(u32::from_be(address.sin_addr.s_addr)),
                netmask: Ipv4Addr::from(u32::from_be(netmask.sin_addr.s_addr)),
                physical,
            });
        }
        cursor = entry.ifa_next;
    }
    // SAFETY: head is the exact list returned by getifaddrs.
    unsafe { libc::freeifaddrs(head) };
    Ok(LocalNetworkSnapshot::new(subnets))
}

#[cfg(not(unix))]
pub(super) fn system_local_network_snapshot() -> io::Result<LocalNetworkSnapshot> {
    Ok(LocalNetworkSnapshot::default())
}

pub(super) fn physical_interface_name(name: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        name.strip_prefix("en").is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
        })
    }
    #[cfg(target_os = "linux")]
    {
        ["en", "eth", "wl", "wlan"]
            .iter()
            .any(|prefix| name.starts_with(prefix))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = name;
        false
    }
}

pub(super) fn route_matches_physical_direct_subnet(
    snapshot: &LocalNetworkSnapshot,
    address: Ipv4Addr,
    interface: Option<&str>,
    uses_gateway: bool,
) -> bool {
    !uses_gateway
        && interface.is_some_and(|interface| {
            snapshot.subnets.iter().any(|subnet| {
                subnet.physical && subnet.interface_name == interface && subnet.contains(address)
            })
        })
}

#[cfg(target_os = "macos")]
pub(super) fn system_route_is_physical_direct(
    snapshot: &LocalNetworkSnapshot,
    address: Ipv4Addr,
) -> io::Result<bool> {
    let output = std::process::Command::new("/sbin/route")
        .args(["-n", "get", &address.to_string()])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    if !output.status.success() {
        return Ok(false);
    }
    let route = String::from_utf8_lossy(&output.stdout);
    let interface = route
        .lines()
        .find_map(|line| line.trim().strip_prefix("interface:").map(str::trim));
    let uses_gateway = route
        .lines()
        .any(|line| line.trim().starts_with("gateway:"));
    Ok(route_matches_physical_direct_subnet(
        snapshot,
        address,
        interface,
        uses_gateway,
    ))
}

#[cfg(target_os = "linux")]
pub(super) fn system_route_is_physical_direct(
    snapshot: &LocalNetworkSnapshot,
    address: Ipv4Addr,
) -> io::Result<bool> {
    let ip = ["/usr/sbin/ip", "/usr/bin/ip"]
        .into_iter()
        .find(|candidate| Path::new(candidate).is_file())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "ip route tool is unavailable"))?;
    let output = std::process::Command::new(ip)
        .args(["-4", "route", "get", &address.to_string()])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    if !output.status.success() {
        return Ok(false);
    }
    let route = String::from_utf8_lossy(&output.stdout);
    let fields = route.split_whitespace().collect::<Vec<_>>();
    let interface = fields
        .windows(2)
        .find_map(|pair| (pair[0] == "dev").then_some(pair[1]));
    let uses_gateway = fields.contains(&"via");
    Ok(route_matches_physical_direct_subnet(
        snapshot,
        address,
        interface,
        uses_gateway,
    ))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(super) fn system_route_is_physical_direct(
    snapshot: &LocalNetworkSnapshot,
    address: Ipv4Addr,
) -> io::Result<bool> {
    Ok(snapshot.authorizes_lan_address(address))
}
