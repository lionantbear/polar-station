//! Host import uses the existing target catalog transaction, never a second store.
use super::*;
use polar_station_protocol::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HostPortability {
    pub catalog_id: String,
    #[serde(default)]
    pub aliases: Vec<HostSourceAlias>,
    #[serde(default)]
    pub observations: BTreeMap<String, LocalObservation>,
    #[serde(default)]
    pub imported_hosts: BTreeMap<String, PortableHostV1>,
}

/// A source identifies a machine, independently of mutable access configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HostSourceAlias {
    pub source: HostRefV1,
    pub target_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<PortableHostConnectionV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_host: Option<PortableHostV1>,
}

/// Local-only evidence. An imported observation can never populate this record.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LocalObservation {
    pub observation: HostIdentityObservationV1,
    pub verified: bool,
    pub connection_details_complete: bool,
}

impl HostPortability {
    pub fn initialized(&self) -> Self {
        let mut result = self.clone();
        if result.catalog_id.is_empty() {
            result.catalog_id = polar_identity::generation::new_uuid_v4().to_string();
        }
        result
    }
}

fn snapshot(state: &TargetRegistryState) -> TargetRegistryFile {
    TargetRegistryFile {
        targets: state.configured_targets.clone(),
        polar_targets: state.polar_targets.clone(),
        portability: state.portability.clone(),
    }
}

fn hosts(config: &TargetRegistryFile) -> Vec<PortableHostV1> {
    let mut result = vec![PortableHostV1 {
        wire_shape: None,
        source: HostRefV1 {
            catalog_id: config.portability.catalog_id.clone(),
            target_id: LOCAL_TARGET_ID.into(),
        },
        display_name: "This computer".into(),
        connection: PortableHostConnectionV1::Local,
        observation: config
            .portability
            .observations
            .get(LOCAL_TARGET_ID)
            .map(|o| o.observation.clone())
            .unwrap_or_default(),
    }];
    result.extend(config.targets.iter().map(|target| {
        PortableHostV1 {
        wire_shape: None,
            source: HostRefV1 {
                catalog_id: config.portability.catalog_id.clone(),
                target_id: target.id().into(),
            },
            display_name: target
                .display_name()
                .cloned()
                .unwrap_or_else(|| target.id().into()),
            connection: PortableHostConnectionV1::Ssh {
                destination: target.destination().into(),
                route_profile: target.route_profile().map(route_profile_to_protocol),
            },
            observation: config
                .portability
                .observations
                .get(target.id())
                .map(|o| o.observation.clone())
                .or_else(|| config.portability.imported_hosts.get(target.id()).map(|host| host.observation.clone()))
                .unwrap_or_default(),
        }
    }));
    result
}

/// Compare configured endpoints only; no DNS or connection is performed.
fn addresses(connection: &PortableHostConnectionV1) -> BTreeSet<(String, u16)> {
    fn address(value: &str) -> String {
        value.parse::<std::net::IpAddr>().map_or_else(|_| value.to_ascii_lowercase(), |ip| ip.to_string())
    }
    match connection {
        PortableHostConnectionV1::Local => BTreeSet::new(),
        PortableHostConnectionV1::Ssh { route_profile: Some(route), .. } => BTreeSet::from([
            (address(&route.lan_address), route.lan_port),
            (address(&route.wan_host), route.wan_port),
        ]),
        PortableHostConnectionV1::Ssh { destination, route_profile: None } =>
            BTreeSet::from([(address(destination.rsplit('@').next().unwrap_or(destination)), 22)]),
    }
}

fn overlaps(a: &PortableHostConnectionV1, b: &PortableHostConnectionV1) -> bool {
    !addresses(a).is_disjoint(&addresses(b))
}

fn duplicate_access_address(a: &PortableHostV1, b: &PortableHostV1) -> bool {
    fn login_username(connection: &PortableHostConnectionV1) -> Option<&str> {
        match connection {
            PortableHostConnectionV1::Ssh { destination, .. } => {
                destination.rsplit_once('@').map(|(username, _)| username)
            }
            PortableHostConnectionV1::Local => None,
        }
    }

    let different_execution_users = a.observation.user.as_ref()
        .zip(b.observation.user.as_ref())
        .is_some_and(|(a, b)| a.name != b.name);
    login_username(&a.connection) == login_username(&b.connection)
        && !different_execution_users
        && overlaps(&a.connection, &b.connection)
}

fn resolve_source(config: &TargetRegistryFile, source: &HostRefV1) -> Option<String> {
    let exists = |id: &str| id == LOCAL_TARGET_ID || config.targets.iter().any(|t| t.id() == id);
    if source.catalog_id == config.portability.catalog_id && exists(&source.target_id) {
        return Some(source.target_id.clone());
    }
    config.portability.aliases.iter().find(|a| a.source == *source && exists(&a.target_id)).map(|a| a.target_id.clone())
}

fn exported_host(config: &TargetRegistryFile, current: PortableHostV1) -> PortableHostV1 {
    let Some(original) = config.portability.imported_hosts.get(&current.source.target_id) else { return current };
    let mut host = original.clone();
    host.display_name = current.display_name;
    host.connection = current.connection;
    host
}

fn validate_bundle(bundle: &HostExportBundleV1) -> Result<()> {
    anyhow::ensure!(
        bundle.schema == HOST_EXPORT_SCHEMA_V1,
        "Unsupported Host export version"
    );
    anyhow::ensure!(
        !bundle.hosts.is_empty() && bundle.hosts.len() <= 256,
        "Choose 1–256 Hosts"
    );
    anyhow::ensure!(
        serde_json::to_vec(bundle)?.len() <= MAX_APPLICATION_PAYLOAD_BYTES / 2,
        "Host export is too large"
    );
    let mut sources = BTreeSet::new();
    for host in &bundle.hosts {
        validate_display_name(&host.display_name)?;
        for id in [&host.source.catalog_id, &host.source.target_id] {
            anyhow::ensure!(
                !id.is_empty() && id.len() <= 128 && !id.chars().any(char::is_control),
                "Invalid Host source reference"
            );
        }
        anyhow::ensure!(
            sources.insert(&host.source),
            "Duplicate Host source reference"
        );
        if let PortableHostConnectionV1::Ssh {
            destination,
            route_profile,
        } = &host.connection
        {
            anyhow::ensure!(
                is_canonical_ssh_destination(destination),
                "Invalid SSH destination in import"
            );
            if let Some(profile) = route_profile {
                route_profile_from_protocol(profile)?;
            }
        }
        if let Some(system) = &host.observation.system {
            anyhow::ensure!(
                !system.kind.trim().is_empty() && !system.value.trim().is_empty(),
                "Incomplete system identity"
            );
        }
        if let Some(user) = &host.observation.user {
            anyhow::ensure!(!user.name.trim().is_empty(), "Incomplete execution user");
        }
        anyhow::ensure!(
            host.observation
                .server_keys
                .iter()
                .all(|k| !k.algorithm.trim().is_empty() && !k.fingerprint.trim().is_empty()),
            "Incomplete SSH server key"
        );
    }
    Ok(())
}

fn field(
    id: &str,
    label: &str,
    local: Option<String>,
    imported: Option<String>,
    identity: bool,
) -> HostFieldComparisonV1 {
    let change = match (&local, &imported) {
        (Some(a), Some(b)) if a == b => HostFieldChangeV1::Equal,
        (Some(_), Some(_)) => HostFieldChangeV1::Changed,
        _ if identity => HostFieldChangeV1::Unknown,
        (None, Some(_)) => HostFieldChangeV1::Added,
        (Some(_), None) => HostFieldChangeV1::Missing,
        (None, None) => HostFieldChangeV1::NotApplicable,
    };
    HostFieldComparisonV1 {
        field: id.into(),
        label: label.into(),
        local,
        imported,
        change,
    }
}

fn config_fields(host: &PortableHostV1) -> BTreeMap<&'static str, Option<String>> {
    let mut fields = BTreeMap::from([("display_name", Some(host.display_name.clone()))]);
    let (destination, route) = match &host.connection {
        PortableHostConnectionV1::Local => (None, None),
        PortableHostConnectionV1::Ssh {
            destination,
            route_profile,
        } => (Some(destination.clone()), route_profile.as_ref()),
    };
    fields.insert("destination", destination);
    fields.insert("lan_address", route.map(|r| r.lan_address.clone()));
    fields.insert("lan_port", route.map(|r| r.lan_port.to_string()));
    fields.insert("wan_host", route.map(|r| r.wan_host.clone()));
    fields.insert("wan_port", route.map(|r| r.wan_port.to_string()));
    fields.insert("doh_providers", route.map(|r| r.doh_providers.join(", ")));
    fields
}

fn keys(host: &PortableHostV1) -> Option<String> {
    let keys = host.observation.server_keys.iter().collect::<BTreeSet<_>>();
    (!keys.is_empty()).then(|| {
        keys.iter()
            .map(|key| format!("{} {}", key.algorithm, key.fingerprint))
            .collect::<Vec<_>>()
            .join("\n")
    })
}

fn compare(
    local: &PortableHostV1,
    imported: &PortableHostV1,
    evidence: Option<&LocalObservation>,
) -> HostImportCandidateV1 {
    let a = &local.observation;
    let b = &imported.observation;
    let mut fields = vec![
        field(
            "system_identity",
            "System identity",
            a.system
                .as_ref()
                .map(|s| format!("{}: {}", s.kind, s.value)),
            b.system
                .as_ref()
                .map(|s| format!("{}: {}", s.kind, s.value)),
            true,
        ),
        field(
            "execution_user",
            "Actual user / UID",
            a.user
                .as_ref()
                .map(|u| format!("{} / UID {}", u.name, u.uid)),
            b.user
                .as_ref()
                .map(|u| format!("{} / UID {}", u.name, u.uid)),
            true,
        ),
        field(
            "server_keys",
            "SSH server keys",
            keys(local),
            keys(imported),
            true,
        ),
        field(
            "hostname",
            "Hostname",
            a.hostname.clone(),
            b.hostname.clone(),
            true,
        ),
        field(
            "platform",
            "Platform",
            a.platform.clone(),
            b.platform.clone(),
            true,
        ),
    ];
    let local_config = config_fields(local);
    for (key, value) in config_fields(imported) {
        fields.push(field(
            key,
            match key {
                "display_name" => "Display name",
                "destination" => "SSH destination",
                "lan_address" => "LAN address",
                "lan_port" => "LAN port",
                "wan_host" => "WAN host",
                "wan_port" => "WAN port",
                _ => "DoH providers",
            },
            local_config[key].clone(),
            value,
            false,
        ));
    }
    let local_only = matches!(local.connection, PortableHostConnectionV1::Local)
        && matches!(imported.connection, PortableHostConnectionV1::Local);
    if local_only {
        fields[2].change = HostFieldChangeV1::NotApplicable;
    }
    // An implicit user's SSH config is not a complete portable connection definition.
    let explicit_direct = |h: &PortableHostV1| {
        matches!(
            h.connection,
            PortableHostConnectionV1::Ssh {
                route_profile: Some(_),
                ..
            }
        )
    };
    let details_complete = local_only
        || (explicit_direct(local)
            && explicit_direct(imported)
            && evidence.is_some_and(|e| e.connection_details_complete));
    fields.push(HostFieldComparisonV1 {
        field: "connection_details".into(),
        label: "Jump path / effective SSH configuration".into(),
        local: explicit_direct(local).then(|| "Direct · explicit LAN/WAN".into()),
        imported: explicit_direct(imported).then(|| "Direct · explicit LAN/WAN".into()),
        change: if local_only {
            HostFieldChangeV1::NotApplicable
        } else if details_complete {
            HostFieldChangeV1::Equal
        } else {
            HostFieldChangeV1::Unknown
        },
    });
    for (id, label, current, incoming) in [
        (
            "catalog_id",
            "Source catalog",
            Some(local.source.catalog_id.clone()),
            Some(imported.source.catalog_id.clone()),
        ),
        (
            "target_id",
            "Source Host reference",
            Some(local.source.target_id.clone()),
            Some(imported.source.target_id.clone()),
        ),
        (
            "observed_at",
            "Observation time",
            a.observed_at.clone(),
            b.observed_at.clone(),
        ),
        (
            "verification",
            "Observation provenance",
            Some(
                if evidence.is_some_and(|e| e.verified) {
                    "Verified locally"
                } else {
                    "Not verified locally"
                }
                .into(),
            ),
            Some("Imported record · not local verification".into()),
        ),
    ] {
        fields.push(HostFieldComparisonV1 {
            field: id.into(),
            label: label.into(),
            local: current,
            imported: incoming,
            change: HostFieldChangeV1::NotApplicable,
        });
    }
    fields.push(field("complete_record", "Complete Host record",
        Some(serde_json::to_string_pretty(local).expect("Host is serializable")),
        Some(serde_json::to_string_pretty(imported).expect("Host is serializable")), false));
    let identity_verified = evidence.is_some_and(|e| e.verified)
        && a.system.is_some()
        && a.system == b.system
        && a.user.is_some()
        && a.user == b.user
        && (local_only || (keys(local).is_some() && keys(local) == keys(imported)))
        && details_complete;
    let different_user = a.user.is_some() && b.user.is_some() && a.user != b.user;
    HostImportCandidateV1 {
        target_id: local.source.target_id.clone(),
        display_name: local.display_name.clone(),
        identity_verified,
        different_user,
        fields,
    }
}

fn plan(config: &TargetRegistryFile, bundle: &HostExportBundleV1) -> Result<HostImportPlanV1> {
    validate_bundle(bundle)?;
    let existing = hosts(config);
    let mut rows = Vec::new();
    for imported in &bundle.hosts {
        let mapped = config
            .portability
            .aliases
            .iter()
            .find(|a| a.source == imported.source)
            .map(|a| a.target_id.as_str());
        let candidates = existing
            .iter()
            .filter(|local| {
                mapped == Some(local.source.target_id.as_str())
                    || local.source == imported.source
                    || (local.observation.system.is_some()
                        && local.observation.system == imported.observation.system)
                    || (keys(local).is_some() && keys(local) == keys(imported))
                    || overlaps(&local.connection, &imported.connection)
                    || local.display_name == imported.display_name
                    || (matches!(local.connection, PortableHostConnectionV1::Ssh { .. })
                        && local.connection == imported.connection)
            })
            .map(|local| {
                compare(
                    local,
                    imported,
                    config.portability.observations.get(&local.source.target_id),
                )
            })
            .collect::<Vec<_>>();
        let previously_associated = config.portability.aliases.iter().any(|alias|
            alias.source == imported.source && alias.imported_host.as_ref().is_some_and(|host|
                serde_json::to_value(host).ok() == serde_json::to_value(imported).ok()));
        let disposition = if previously_associated && resolve_source(config, &imported.source).is_some() {
            HostImportDispositionV1::Identical
        } else if existing.iter().any(|local| duplicate_access_address(local, imported)) {
            HostImportDispositionV1::DuplicateAddress
        } else if candidates.is_empty() {
            HostImportDispositionV1::New
        } else {
            HostImportDispositionV1::Review
        };
        let fields = compare(imported, imported, None)
            .fields
            .into_iter()
            .map(|mut field| {
                field.local = None;
                field.change = if field.imported.is_some() {
                    HostFieldChangeV1::Added
                } else {
                    HostFieldChangeV1::Unknown
                };
                field
            })
            .collect();
        rows.push(HostImportRowV1 {
            fields,
            host: imported.clone(),
            disposition,
            candidates,
        });
    }
    let result = HostImportPlanV1 {
        targets: existing
            .iter()
            .map(|h| HostImportTargetV1 {
                target_id: h.source.target_id.clone(),
                display_name: h.display_name.clone(),
            })
            .collect(),
        revision: connections::revision(config),
        rows,
    };
    anyhow::ensure!(
        serde_json::to_vec(&result)?.len() <= MAX_APPLICATION_PAYLOAD_BYTES / 2,
        "Host comparisons exceed the transfer limit; import fewer Hosts at a time"
    );
    Ok(result)
}

impl TargetRegistry {
    /// Explicit export establishes the stable catalog identity if needed. Read-only load never does.
    pub fn export_hosts(&self, target_ids: &[String]) -> Result<HostExportBundleV1> {
        anyhow::ensure!(!target_ids.is_empty(), "Select at least one Host to export");
        let path = self
            .config_path
            .as_deref()
            .context("Host export requires persistent Connections")?;
        let _guard = self
            .update_gate
            .lock()
            .expect("target update lock poisoned");
        let mut state = self.state.write().expect("target registry lock poisoned");
        let config = snapshot(&state);
        let available = hosts(&config);
        anyhow::ensure!(
            target_ids
                .iter()
                .all(|id| available.iter().any(|h| &h.source.target_id == id)),
            "Selected Host no longer exists"
        );
        // Legacy non-default socket targets cannot be silently reinterpreted as default tmux.
        anyhow::ensure!(
            !config
                .targets
                .iter()
                .any(|t| target_ids.iter().any(|id| id == t.id()) && t.socket_name().is_some()),
            "Only default tmux Hosts can be exported"
        );
        if state.portability.catalog_id.is_empty() {
            self.commit_configuration(&mut state, config, path)?;
        }
        let bundle = HostExportBundleV1 {
            schema: HOST_EXPORT_SCHEMA_V1.into(),
            hosts: hosts(&snapshot(&state))
                .into_iter()
                .filter(|h| target_ids.contains(&h.source.target_id))
                .map(|h| exported_host(&snapshot(&state), h))
                .collect(),
        };
        validate_bundle(&bundle)?;
        Ok(bundle)
    }

    pub fn plan_host_import(&self, bundle: &HostExportBundleV1) -> Result<HostImportPlanV1> {
        plan(
            &snapshot(&self.state.read().expect("target registry lock poisoned")),
            bundle,
        )
    }

    /// Only the local target observer calls this; imported claims never confer verification.
    pub(crate) fn record_host_observation(
        &self,
        target_id: &str,
        configuration_revision: u64,
        observation: HostIdentityObservationV1,
    ) -> Result<()> {
        let path = self
            .config_path
            .as_deref()
            .context("Host observations require persistent Connections")?;
        let _guard = self
            .update_gate
            .lock()
            .expect("target update lock poisoned");
        let mut state = self.state.write().expect("target registry lock poisoned");
        let target = state
            .targets
            .get(target_id)
            .context("Observed Host is no longer configured")?;
        anyhow::ensure!(
            target.configuration_revision() == configuration_revision,
            "Host connection changed during identity observation"
        );
        let mut config = snapshot(&state);
        config.portability.observations.insert(
            target_id.into(),
            LocalObservation {
                observation,
                verified: true,
                connection_details_complete: target_id == LOCAL_TARGET_ID,
            },
        );
        self.commit_configuration(&mut state, config, path)
    }

    /// Manually selected candidates use the same comparison and catalog revision as matches.
    pub fn compare_host_import(
        &self,
        expected_revision: &str,
        host: &PortableHostV1,
        target_id: &str,
    ) -> Result<HostImportCandidateV1> {
        validate_bundle(&HostExportBundleV1 {
            schema: HOST_EXPORT_SCHEMA_V1.into(),
            hosts: vec![host.clone()],
        })?;
        let state = self.state.read().expect("target registry lock poisoned");
        let config = snapshot(&state);
        anyhow::ensure!(
            connections::revision(&config) == expected_revision,
            "Connections changed; review the import again"
        );
        let local = hosts(&config)
            .into_iter()
            .find(|h| h.source.target_id == target_id)
            .context("Selected Host no longer exists")?;
        Ok(compare(
            &local,
            host,
            config.portability.observations.get(target_id),
        ))
    }

    pub fn resolve_host_ref(&self, source: &HostRefV1) -> Option<String> {
        let state = self.state.read().expect("target registry lock poisoned");
        resolve_source(&snapshot(&state), source)
    }

    /// Preflight produces ordinary connection edits so every surface uses existing reference checks.
    pub fn host_import_connection_changes(
        &self,
        expected_revision: &str,
        bundle: &HostExportBundleV1,
        decisions: &[HostImportDecisionV1],
    ) -> Result<Vec<ConnectionChangeV1>> {
        let state = self.state.read().expect("target registry lock poisoned");
        let (config, result) =
            prepare_import(snapshot(&state), expected_revision, bundle, decisions)?;
        Ok(config
            .targets
            .iter()
            .filter(|target| result.changed.iter().any(|id| id == target.id()))
            .map(|target| ConnectionChangeV1::SaveMachine {
                machine: MachineConnectionV1 {
                    id: target.id().into(),
                    display_name: target
                        .display_name()
                        .cloned()
                        .unwrap_or_else(|| target.id().into()),
                    destination: target.destination().into(),
                    route_profile: target.route_profile().map(route_profile_to_protocol),
                },
            })
            .collect())
    }

    pub fn apply_host_import(
        &self,
        expected_revision: &str,
        bundle: &HostExportBundleV1,
        decisions: &[HostImportDecisionV1],
    ) -> Result<HostImportResultV1> {
        let path = self
            .config_path
            .as_deref()
            .context("Host import requires persistent Connections")?;
        let _guard = self
            .update_gate
            .lock()
            .expect("target update lock poisoned");
        let mut state = self.state.write().expect("target registry lock poisoned");
        let (config, mut result) =
            prepare_import(snapshot(&state), expected_revision, bundle, decisions)?;
        if !result.associations.is_empty() {
            self.commit_configuration(&mut state, config, path)?;
        }
        result.revision = connections::revision(&snapshot(&state));
        Ok(result)
    }
}

fn prepare_import(
    mut config: TargetRegistryFile,
    expected_revision: &str,
    bundle: &HostExportBundleV1,
    decisions: &[HostImportDecisionV1],
) -> Result<(TargetRegistryFile, HostImportResultV1)> {
    let review = plan(&config, bundle)?;
    anyhow::ensure!(
        review.revision == expected_revision,
        "Connections changed; review the import again"
    );
    anyhow::ensure!(
        decisions.len() == review.rows.len(),
        "Choose an action for every imported Host"
    );
    let mut handled = BTreeSet::new();
    let mut result = HostImportResultV1 {
        revision: review.revision.clone(),
        associations: Vec::new(),
        created: Vec::new(),
        changed: Vec::new(),
        skipped: Vec::new(),
    };
    for decision in decisions {
        let source = decision.source();
        anyhow::ensure!(handled.insert(source), "Duplicate import decision");
        let row = review
            .rows
            .iter()
            .find(|r| &r.host.source == source)
            .context("Unknown import source")?;
        let target_id = match decision {
            HostImportDecisionV1::Skip { .. } => {
                result.skipped.push(source.clone());
                continue;
            }
            HostImportDecisionV1::Automatic { .. } => {
                anyhow::ensure!(
                    row.disposition == HostImportDispositionV1::Identical,
                    "This Host needs review"
                );
                resolve_source(&config, source).context("Source association no longer exists")?
            }
            HostImportDecisionV1::Create { .. } => {
                if resolve_source(&config, source).is_some()
                    || hosts(&config).iter().any(|local| duplicate_access_address(local, &row.host)) {
                    result.skipped.push(source.clone());
                    continue;
                }
                let restoring_original = source.catalog_id == config.portability.catalog_id;
                anyhow::ensure!(
                    !restoring_original || !hosts(&config).iter().any(|h| h.source == *source),
                    "This is already a Host in the current catalog"
                );
                let PortableHostConnectionV1::Ssh {
                    destination,
                    route_profile,
                } = &row.host.connection
                else {
                    bail!("Configure an SSH Host for the original machine and user, then associate this source-local Host")
                };
                let id = if restoring_original {
                    source.target_id.clone()
                } else {
                    format!("machine-{}", polar_identity::generation::new_uuid_v4())
                };
                config.targets.push(ConfiguredTarget::Ssh {
                    id: id.clone(),
                    display_name: Some(row.host.display_name.clone()),
                    destination: destination.clone(),
                    route_profile: route_profile
                        .as_ref()
                        .map(route_profile_from_protocol)
                        .transpose()?,
                    socket_name: None,
                    workspace_id: None,
                });
                config.portability.imported_hosts.insert(id.clone(), row.host.clone());
                result.created.push(id.clone());
                id
            }
            HostImportDecisionV1::Merge {
                target_id,
                fields,
                allow_address_removal,
                ..
            } => {
                anyhow::ensure!(fields.is_empty() && !allow_address_removal,
                    "Host association cannot replace fields or remove addresses");
                anyhow::ensure!(hosts(&config).iter().any(|h| h.source.target_id == *target_id),
                    "Selected Host no longer exists");
                target_id.clone()
            }
        };
        if source.catalog_id == config.portability.catalog_id {
            anyhow::ensure!(
                source.target_id == target_id,
                "Cannot reassign this catalog's original Host reference"
            );
        }
        if let Some(alias) = config
            .portability
            .aliases
            .iter_mut()
            .find(|a| &a.source == source)
        {
            anyhow::ensure!(
                alias.target_id == target_id,
                "Existing Host source association cannot be reassigned"
            );
            alias.connection = None;
            alias.imported_host = Some(row.host.clone());
        } else {
            config.portability.aliases.push(HostSourceAlias {
                source: source.clone(),
                target_id: target_id.clone(),
                connection: None,
                imported_host: Some(row.host.clone()),
            });
        }
        result.associations.push(HostImportAssociationV1 {
            source: source.clone(),
            target_id,
        });
    }
    Ok((config, result))
}


