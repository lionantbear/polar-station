use anyhow::{Context, Result};
use mdns_sd::{ServiceDaemon, ServiceInfo};

pub const SERVICE_TYPE: &str = "_polar-tmux._tcp.local.";
pub const WEBSOCKET_PATH: &str = "/v1/ws";
pub const AUTH_MODE: &str = "hmac-hkdf-chacha20poly1305";

pub struct DiscoveryAdvertisement {
    daemon: ServiceDaemon,
    fullname: String,
}

impl DiscoveryAdvertisement {
    pub fn advertise(host_id: &str, port: u16) -> Result<Self> {
        let service = service_info(host_id, port)?;
        let fullname = service.get_fullname().to_string();
        let daemon = ServiceDaemon::new().context("failed to start mDNS daemon")?;
        daemon
            .register(service)
            .context("failed to advertise Polar Tmux Host")?;
        Ok(Self { daemon, fullname })
    }
}

impl Drop for DiscoveryAdvertisement {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

fn service_info(host_id: &str, port: u16) -> Result<ServiceInfo> {
    let short_id = host_id
        .chars()
        .filter(|value| *value != '-')
        .take(12)
        .collect::<String>();
    let host_name = format!("polar-tmux-{short_id}.local.");
    let instance_name = format!("Polar Tmux {short_id}");
    let properties = [
        ("host_id", host_id),
        ("v", "1"),
        ("path", WEBSOCKET_PATH),
        ("auth", AUTH_MODE),
    ];
    ServiceInfo::new(
        SERVICE_TYPE,
        &instance_name,
        &host_name,
        "",
        port,
        &properties[..],
    )
    .context("failed to construct Polar Tmux discovery record")
    .map(ServiceInfo::enable_addr_auto)
}


