//! Shared import interaction; native surfaces only pick files and present the plan.
use super::*;
use polar_station_protocol::{HostImportDispositionV1, PortableHostConnectionV1};

impl ConnectionsEngine {
    pub(super) fn can_apply_import(&self) -> bool {
        self.pending.is_none()
            && self.transfer_plan.as_ref().is_some_and(|plan| {
                plan.revision == self.facts.revision
                    && plan.rows.len() == self.transfer_decisions.len()
                    && plan.rows.iter().all(|row| {
                        self.transfer_decisions
                            .iter()
                            .any(|d| d.source() == &row.host.source)
                    })
            })
    }
    fn request_transfer(&mut self, request: HostTransferRequestV1) -> Vec<ConnectionsEffect> {
        self.next_token += 1;
        self.pending = Some(self.next_token);
        self.transfer_request = Some(request.clone());
        self.message = Some("Working…".into());
        vec![ConnectionsEffect::HostTransfer {
            token: self.next_token,
            request,
        }]
    }
    pub(super) fn dispatch_transfer(
        &mut self,
        input: &ConnectionsInput,
    ) -> Option<Vec<ConnectionsEffect>> {
        if let ConnectionsInput::TransferComplete {
            token,
            result,
            error,
        } = input
        {
            if self.pending != Some(*token) || self.transfer_request.is_none() {
                return Some(vec![]);
            }
            self.pending = None;
            let request = self
                .transfer_request
                .take()
                .expect("checked transfer request");
            if let Some(error) = error {
                self.message = Some(error.clone());
                return Some(vec![]);
            }
            match (request, result) {
                (
                    HostTransferRequestV1::Export { .. },
                    Some(HostTransferResponseV1::Exported { bundle }),
                ) => {
                    self.message = None;
                    return Some(vec![ConnectionsEffect::WriteHostExport {
                        bundle: bundle.clone(),
                    }]);
                }
                (
                    HostTransferRequestV1::Plan { .. },
                    Some(HostTransferResponseV1::Planned { plan }),
                ) => {
                    self.transfer_decisions = plan
                        .rows
                        .iter()
                        .filter_map(|row| match row.disposition {
                            HostImportDispositionV1::Identical => {
                                Some(HostImportDecisionV1::Automatic {
                                    source: row.host.source.clone(),
                                })
                            }
                            HostImportDispositionV1::DuplicateAddress => Some(HostImportDecisionV1::Skip { source: row.host.source.clone() }),
                            HostImportDispositionV1::New
                                if matches!(
                                    row.host.connection,
                                    PortableHostConnectionV1::Ssh { .. }
                                ) =>
                            {
                                Some(HostImportDecisionV1::Create {
                                    source: row.host.source.clone(),
                                })
                            }
                            _ => None,
                        })
                        .collect();
                    self.transfer_plan = Some(plan.clone());
                    self.message =
                        Some("Duplicate addresses are skipped. Select an existing Host to save an association; existing settings stay unchanged.".into());
                }
                (
                    HostTransferRequestV1::Compare {
                        host,
                        target_id,
                        expected_revision,
                    },
                    Some(HostTransferResponseV1::Compared {
                        source,
                        revision,
                        candidate,
                    }),
                ) if host.source == *source
                    && target_id == candidate.target_id
                    && expected_revision == *revision =>
                {
                    if let Some(plan) = &mut self.transfer_plan {
                        if plan.revision == *revision && self.facts.revision == *revision {
                            if let Some(row) =
                                plan.rows.iter_mut().find(|r| r.host.source == *source)
                            {
                                row.candidates
                                    .retain(|c| c.target_id != candidate.target_id);
                                row.candidates.push(candidate.clone());
                            }
                            self.message = Some(
                                "Compare the selected Host before choosing Use this Host.".into(),
                            );
                        } else {
                            self.message = Some(
                                "Connections changed; reopen the import before applying it.".into(),
                            );
                        }
                    }
                }
                (
                    HostTransferRequestV1::Apply { .. },
                    Some(HostTransferResponseV1::Applied { result }),
                ) => {
                    self.message = Some(format!("Imported: {} new, {} associated, {} skipped. Configure authorization for new Hosts before connecting.", result.created.len(), result.associations.len().saturating_sub(result.created.len()), result.skipped.len()));
                    self.transfer_plan = None;
                    self.transfer_bundle = None;
                    self.transfer_decisions.clear();
                }
                _ => {
                    self.message = Some(
                        "Unexpected Host transfer response. No automatic retry was sent.".into(),
                    )
                }
            }
            return Some(vec![]);
        }
        if !matches!(
            input,
            ConnectionsInput::ExportHosts { .. }
                | ConnectionsInput::ImportHosts { .. }
                | ConnectionsInput::CompareHostImport { .. }
                | ConnectionsInput::ChooseHostImport { .. }
                | ConnectionsInput::ApplyHostImport
                | ConnectionsInput::CancelHostImport
        ) {
            return None;
        }
        if self.pending.is_some() {
            return Some(vec![]);
        }
        if self.editing() {
            self.message = Some(
                "Save or cancel the connection draft before importing or exporting Hosts.".into(),
            );
            return Some(vec![]);
        }
        if !self.facts.host_transfer_supported {
            self.message =
                Some("This Host needs an update to support Host import and export.".into());
            return Some(vec![]);
        }
        match input {
            ConnectionsInput::ExportHosts { target_ids } => {
                Some(self.request_transfer(HostTransferRequestV1::Export {
                    target_ids: target_ids.clone(),
                }))
            }
            ConnectionsInput::ImportHosts { bundle } => {
                self.transfer_bundle = Some(bundle.clone());
                self.transfer_plan = None;
                self.transfer_decisions.clear();
                Some(self.request_transfer(HostTransferRequestV1::Plan {
                    bundle: bundle.clone(),
                }))
            }
            ConnectionsInput::CancelHostImport => {
                self.transfer_bundle = None;
                self.transfer_plan = None;
                self.transfer_decisions.clear();
                self.message = None;
                Some(vec![])
            }
            ConnectionsInput::CompareHostImport { source, target_id } => {
                let Some(plan) = &self.transfer_plan else {
                    return Some(vec![]);
                };
                let Some(row) = plan.rows.iter().find(|r| r.host.source == *source) else {
                    return Some(vec![]);
                };
                self.transfer_decisions
                    .retain(|decision| decision.source() != source);
                if row
                    .candidates
                    .iter()
                    .any(|candidate| candidate.target_id == *target_id)
                {
                    self.message =
                        Some("Review the selected Host, then choose Use this Host.".into());
                    return Some(vec![]);
                }
                let request = HostTransferRequestV1::Compare {
                    expected_revision: plan.revision.clone(),
                    host: Box::new(row.host.clone()),
                    target_id: target_id.clone(),
                };
                Some(self.request_transfer(request))
            }
            ConnectionsInput::ChooseHostImport { decision } => {
                if let Some(plan) = &self.transfer_plan {
                    if plan
                        .rows
                        .iter()
                        .any(|r| &r.host.source == decision.source())
                    {
                        self.transfer_decisions
                            .retain(|d| d.source() != decision.source());
                        self.transfer_decisions.push(decision.clone());
                    }
                }
                Some(vec![])
            }
            ConnectionsInput::ApplyHostImport => {
                if !self.can_apply_import() {
                    self.message = Some(
                        "Resolve every Host and reload the import if Connections changed.".into(),
                    );
                    return Some(vec![]);
                }
                Some(
                    self.request_transfer(HostTransferRequestV1::Apply {
                        expected_revision: self
                            .transfer_plan
                            .as_ref()
                            .expect("review checked")
                            .revision
                            .clone(),
                        bundle: self.transfer_bundle.clone().expect("review has bundle"),
                        decisions: self.transfer_decisions.clone(),
                    }),
                )
            }
            _ => unreachable!("classified transfer input"),
        }
    }
}


