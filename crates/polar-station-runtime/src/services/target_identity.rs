//! Read-only execution identity on the existing target transport.
//! This does not discover credentials, accept SSH keys or infer identity from addresses.
use anyhow::{Context, Result};
use polar_station_protocol::{
    HostExecutionUserV1, HostIdentityObservationV1, HostSystemIdentityV1,
};
use polar_tmux_core::TmuxTarget;
use sha2::{Digest, Sha256};

const SCRIPT: &str = r#"
station_identity_platform=$(uname -s) || exit 1
station_identity_host=$(hostname) || station_identity_host=
station_identity_user=$(id -un) || exit 1
station_identity_uid=$(id -u) || exit 1
station_identity_kind=
station_identity_machine=
case "$station_identity_platform" in
  Linux)
    station_identity_kind=linux-machine-id
    station_identity_machine=$(cat /etc/machine-id 2>/dev/null) || station_identity_machine=
    ;;
  Darwin)
    station_identity_kind=darwin-io-platform-uuid
    station_identity_machine=$(/usr/sbin/ioreg -rd1 -c IOPlatformExpertDevice 2>/dev/null | awk -F '"' '/"IOPlatformUUID"/ {print $(NF-1); exit}')
    ;;
esac
printf '%s\000' "$station_identity_platform" "$station_identity_host" "$station_identity_user" "$station_identity_uid" "$station_identity_kind" "$station_identity_machine"
"#;

pub(super) async fn read(target: &TmuxTarget) -> Result<HostIdentityObservationV1> {
    let bytes = super::target_directory::target_output(target, SCRIPT).await?;
    let observed_at =
        time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339)?;
    parse(&bytes, observed_at)
}

fn parse(bytes: &[u8], observed_at: String) -> Result<HostIdentityObservationV1> {
    anyhow::ensure!(bytes.len() <= 8192, "Host identity response is too large");
    let text = std::str::from_utf8(bytes).context("Invalid Host identity encoding")?;
    let fields: Vec<_> = text.split('\0').collect();
    anyhow::ensure!(
        fields.len() == 7 && fields[6].is_empty(),
        "Invalid Host identity response"
    );
    anyhow::ensure!(
        fields
            .iter()
            .all(|s| s.len() <= 1024 && !s.chars().any(char::is_control)),
        "Invalid Host identity field"
    );
    anyhow::ensure!(
        !fields[0].is_empty() && !fields[2].is_empty(),
        "Host platform or execution user is missing"
    );
    let uid = fields[3].parse::<u32>().context("Invalid execution UID")?;
    let raw_id = fields[5].to_ascii_lowercase();
    // Unavailable, all-zero or malformed system IDs remain unknown. Never use a hostname fallback.
    let valid_id = match (fields[0], fields[4]) {
        ("Linux", "linux-machine-id") => {
            raw_id.len() == 32 && raw_id.bytes().all(|b| b.is_ascii_hexdigit())
        }
        ("Darwin", "darwin-io-platform-uuid") => {
            raw_id.len() == 36
                && raw_id.bytes().enumerate().all(|(i, b)| {
                    if [8, 13, 18, 23].contains(&i) {
                        b == b'-'
                    } else {
                        b.is_ascii_hexdigit()
                    }
                })
        }
        _ => false,
    } && raw_id.bytes().any(|b| b != b'0' && b != b'-');
    let system = valid_id.then(|| {
        let mut hash = Sha256::new();
        hash.update(b"polar-station/host-system-identity/v1\0");
        hash.update(fields[4].as_bytes());
        hash.update([0]);
        hash.update(raw_id.as_bytes());
        HostSystemIdentityV1 {
            kind: format!("{}:polar-station-sha256-v1", fields[4]),
            value: format!("{:x}", hash.finalize()),
        }
    });
    Ok(HostIdentityObservationV1 {
        system,
        user: Some(HostExecutionUserV1 {
            name: fields[2].into(),
            uid,
        }),
        // OpenSSH's manager currently exposes no verified negotiated-key observation.
        server_keys: vec![],
        hostname: (!fields[1].is_empty()).then(|| fields[1].into()),
        platform: Some(fields[0].into()),
        observed_at: Some(observed_at),
    })
}


