//! A fixed Inquiry form binds a human-confirmed task to an observed terminal.
//! Inquiry remains the only interaction store; Host remains the input owner.
use crate::*;
use polar_inquiry_graph_model::{
    InquiryGraphAnswerKind, InquiryGraphPackage, InquiryGraphResultArtifact,
    InquiryGraphSessionSource, InquiryGraphSessionState, InquiryGraphSessionStatus,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const METADATA_KEY: &str = "station_session_dispatch";
pub const FORM_SCHEMA: &str = "polar.station.session_dispatch.v1";
pub const TEXT_NODE: &str = "task_text";
pub const TARGET_NODE: &str = "target_session";

/// No invitation or terminal effect has occurred when this rejection is returned.
#[derive(Debug)]
pub struct CandidateReferenceMismatch;
impl std::fmt::Display for CandidateReferenceMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("candidate changed or was not returned by the real Host")
    }
}
impl std::error::Error for CandidateReferenceMismatch {}


#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutingContext {
    pub owner: String,
    pub thread: String,
    pub actor: Option<String>,
    pub source_sender: String,
    pub source_message: String,
}
impl RoutingContext {
    fn validate(&self) -> Result<()> {
        for s in [
            &self.owner,
            &self.thread,
            &self.source_sender,
            &self.source_message,
        ] {
            identifier(s)?;
        }
        if let Some(actor) = &self.actor {
            identifier(actor)?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Recommendation {
    pub reference: SessionReference,
    pub reason: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OfferRequest {
    #[serde(default)]
    pub title: Option<String>,
    pub text: String,
    pub candidates: Vec<Recommendation>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfirmedRequest {
    pub inquiry_session_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invocation<T> {
    pub context: RoutingContext,
    pub input: T,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OfferReceipt {
    pub sender_id: String,
    pub message_id: String,
    pub package_id: String,
}
pub struct ReceiptEvent {
    pub command_id: String,
    pub body: Value,
    pub created_at_secs: u64,
}

/// Facts read through the existing Inquiry store and canonical LGP messages.
#[derive(Clone)]
pub struct Confirmation {
    pub package: InquiryGraphPackage,
    pub invitation_package: InquiryGraphPackage,
    pub invitation_actor: Option<String>,
    pub invitation_sender: String,
    pub session: InquiryGraphSessionState,
    pub result: InquiryGraphResultArtifact,
    pub submitted: bool,
    pub formal_result: Option<InquiryGraphResultArtifact>,
    pub formal_author_kind: String,
}

#[async_trait::async_trait]
pub trait RoutingAuthority: Send + Sync {
    async fn publish_offer(
        &self,
        context: &RoutingContext,
        package: &InquiryGraphPackage,
        command_id: &str,
    ) -> Result<OfferReceipt>;
    async fn confirmation(&self, context: &RoutingContext, id: &str) -> Result<Confirmation>;
    async fn publish_receipt(&self, context: &RoutingContext, event: &ReceiptEvent) -> Result<()>;
}

pub fn form_metadata(metadata: &Value) -> Option<&Value> {
    metadata
        .get(METADATA_KEY)
        .filter(|v| v["schema"] == FORM_SCHEMA)
}
/// An explicit human request returns to the same conversation; it never sends terminal input.
pub fn other_sessions_request(
    package: &InquiryGraphPackage,
    owner: &str,
    thread: &str,
    text: &str,
) -> Result<Value> {
    validate_terminal_text(text)?;
    let metadata = form_metadata(&package.metadata).context("not a Session dispatch form")?;
    ensure!(
        metadata["context"]["owner"] == owner && metadata["context"]["thread"] == thread,
        "form belongs to another conversation"
    );
    let previous = package
        .nodes
        .iter()
        .find(|n| n.node_id == TARGET_NODE)
        .context("missing candidates")?
        .choices
        .iter()
        .map(|c| c.label.clone())
        .collect::<Vec<_>>();
    Ok(
        json!({"message_kind":"station.session.recommendation.request","text":format!("请重新查询当前可用的 Session，为下面的任务推荐其他合适的会话，并重新提供确认表单。此前候选：{}。任务：\n{text}",previous.join("、")),"task_text":text,"previous_candidates":previous}),
    )
}
fn digest(value: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
pub struct RoutingService {
    sessions: Arc<SessionRuntime>,
    authority: Arc<dyn RoutingAuthority>,
    publication: tokio::sync::Mutex<()>,
}
impl RoutingService {
    pub fn new(sessions: Arc<SessionRuntime>, authority: Arc<dyn RoutingAuthority>) -> Self {
        Self {
            sessions,
            authority,
            publication: tokio::sync::Mutex::new(()),
        }
    }
    pub async fn offer(
        &self,
        context: RoutingContext,
        request: OfferRequest,
    ) -> Result<OfferReceipt> {
        context.validate()?;
        ensure!(
            context.actor.is_some(),
            "Session recommendations require an equipped inhouse Polar"
        );
        validate_terminal_text(&request.text)?;
        ensure!(
            (1..=3).contains(&request.candidates.len()),
            "recommend one to three real terminal panes"
        );
        let catalog = self.sessions.list(ListRequest::default()).await?;
        let mut choices = vec![];
        let mut seen = std::collections::BTreeSet::new();
        for (index, candidate) in request.candidates.iter().enumerate() {
            candidate.reference.validate().map_err(|_| CandidateReferenceMismatch)?;
            ensure!(
                !candidate.reason.trim().is_empty() && candidate.reason.len() <= 1024,
                "a short recommendation reason is required"
            );
            ensure!(
                seen.insert(serde_json::to_string(&candidate.reference)?),
                "duplicate candidate"
            );
            let (session, pane) = catalog
                .sessions
                .iter()
                .find_map(|session| {
                    session
                        .panes
                        .iter()
                        .find(|p| p.available && p.reference == candidate.reference)
                        .map(|p| (session, p))
                })
                .ok_or(CandidateReferenceMismatch)?;
            let target = catalog
                .targets
                .iter()
                .find(|t| t.target_id == session.target_id && t.available)
                .context("candidate Host is unavailable")?;
            let title = session.agent.as_ref().map(|a| a.title.as_str()).unwrap_or(&session.name);
            choices.push(json!({"choice_id":format!("candidate-{}", index+1), "label":title,
                "value":candidate.reference, "metadata":{"host":format!("{} · {}",catalog.source_host_name,target.label),"directory":pane.working_directory,"reason":candidate.reason.trim(),"agent_type":session.kind,"summary":session.summary,"session_name":session.name,"preview":session.agent.as_ref().and_then(|a|a.preview.as_deref()),"status":session.agent.as_ref().map(|a|a.status.as_str()),"updated_at":session.agent.as_ref().and_then(|a|a.updated_at)}}));
        }
        let id = digest(&(&context, &request))?;
        let package: InquiryGraphPackage = serde_json::from_value(json!({
            "manifest":{"package_id":format!("station-session-{id}"),"package_version":"1.0.0","package_kind":"diagnostic",
                "schema_version":polar_inquiry_graph_model::INQUIRY_GRAPH_PACKAGE_SCHEMA_VERSION,"title":request.title.as_deref().filter(|t|!t.trim().is_empty()).unwrap_or(&request.text).split_whitespace().collect::<Vec<_>>().join(" ").chars().take(120).collect::<String>(),"entry_node_id":TEXT_NODE,
                "required_capabilities":["long_text","scenario_choice"]},
            "metadata":{METADATA_KEY:{"schema":FORM_SCHEMA,"context":context,"draft_text":request.text}},
            "nodes":[{"node_id":TEXT_NODE,"kind":"long_text","title":"Task","prompt":"Edit the exact text to send.","fields":[{"field_id":"text","label":"Task","input_kind":"textarea","max_length":8192}]},
                {"node_id":TARGET_NODE,"kind":"scenario_choice","title":"Choose a Session","choices":choices}],
            "edges":[{"edge_id":"task-to-session","from_node_id":TEXT_NODE,"to_node_id":TARGET_NODE}]
        }))?;
        let _publication = self.publication.lock().await;
        self.authority
            .publish_offer(&context, &package, &format!("station-session-offer:{id}"))
            .await
    }
    pub async fn send_confirmed(
        &self,
        context: RoutingContext,
        request: ConfirmedRequest,
    ) -> Result<Receipt> {
        context.validate()?;
        identifier(&request.inquiry_session_id)?;
        let fact = self
            .authority
            .confirmation(&context, &request.inquiry_session_id)
            .await?;
        let metadata =
            form_metadata(&fact.package.metadata).context("not a Session dispatch Inquiry")?;
        let issuer: RoutingContext = serde_json::from_value(metadata["context"].clone())?;
        ensure!(
            issuer.owner == context.owner && issuer.thread == context.thread,
            "Inquiry belongs to another conversation"
        );
        ensure!(
            context
                .actor
                .as_ref()
                .is_none_or(|actor| Some(actor) == issuer.actor.as_ref()),
            "Inquiry belongs to another inhouse Polar"
        );
        ensure!(
            fact.invitation_actor == issuer.actor
                && fact.invitation_sender == issuer.owner
                && fact.invitation_package == fact.package,
            "canonical invitation differs from the interaction snapshot"
        );
        ensure!(
            matches!(&fact.session.source, Some(InquiryGraphSessionSource::LgpOffer{owner, thread, ..}) if owner == &context.owner && thread == &context.thread),
            "Inquiry invitation scope mismatch"
        );
        ensure!(
            fact.submitted
                && fact.session.status == InquiryGraphSessionStatus::Completed
                && fact.result.session_id == request.inquiry_session_id
                && fact.session.session_id == request.inquiry_session_id,
            "Inquiry has not been formally submitted"
        );
        ensure!(
            fact.session.package_id == fact.package.manifest.package_id
                && fact.session.package_version == fact.package.manifest.package_version
                && fact.result.package_id == fact.session.package_id
                && fact.result.package_version == fact.session.package_version
                && fact.result.answers == fact.session.answers,
            "canonical result differs from the completed interaction"
        );
        ensure!(
            fact.formal_author_kind == "local_human"
                && fact.formal_result.as_ref() == Some(&fact.result),
            "canonical human result is unavailable"
        );
        let answer = fact
            .result
            .answers
            .iter()
            .find(|a| {
                a.node_id == TEXT_NODE
                    && a.answer_kind == InquiryGraphAnswerKind::Text
                    && !a.unknown
            })
            .context("confirmed task text is missing")?;
        let selection = fact
            .result
            .answers
            .iter()
            .find(|a| {
                a.node_id == TARGET_NODE
                    && a.answer_kind == InquiryGraphAnswerKind::Choice
                    && !a.unknown
                    && a.choice_ids.len() == 1
            })
            .context("one confirmed Session is required")?;
        let choice = fact
            .package
            .nodes
            .iter()
            .find(|n| n.node_id == TARGET_NODE)
            .and_then(|n| {
                n.choices
                    .iter()
                    .find(|c| Some(&c.choice_id) == selection.choice_ids.first())
            })
            .context("confirmed candidate is not in the invitation")?;
        let reference: SessionReference = serde_json::from_value(choice.value.clone())?;
        let intent_id = format!(
            "station-route:{}",
            digest(&(&context.owner, &context.thread, &request.inquiry_session_id))?
        );
        let principal = serde_json::to_string(&(
            &issuer.owner,
            Some((
                "inhouse_polar",
                issuer
                    .actor
                    .as_deref()
                    .context("invitation has no inhouse author")?,
            )),
        ))?;
        let receipt = self
            .sessions
            .send(
                &principal,
                SendRequest {
                    intent_id: intent_id.clone(),
                    reference: reference.clone(),
                    text: answer.text.clone().context("confirmed task is empty")?,
                    press_enter: true,
                },
            )
            .await
            .unwrap_or_else(|_| Receipt {
                intent_id,
                reference,
                state: DeliveryStatus::DeliveryUnknown,
                error: Some("terminal_delivery_unconfirmed".into()),
            });
        let label = match receipt.state {
            DeliveryStatus::Delivered => "Terminal input delivered",
            DeliveryStatus::FailedBeforeWrite => "Failed before terminal input",
            _ => "Terminal delivery unknown; check the receipt before sending again",
        };
        let body = json!({"message_kind":"station.session.delivery.receipt", "text":format!("{label} · {} · {}",choice.metadata["host"].as_str().unwrap_or("Host"),choice.label),
            "inquiry_session_id":request.inquiry_session_id,"target_label":choice.label,"receipt":receipt});
        let event = ReceiptEvent {
            command_id: format!("station-route-receipt:{}", digest(&body)?),
            body,
            created_at_secs: fact.result.completed_at_secs.max(0) as u64,
        };
        let mut reply_context = context;
        if let Some(InquiryGraphSessionSource::LgpOffer {
            source_key: Some(source),
            ..
        }) = &fact.session.source
        {
            let (sender, message) = source
                .split_once(':')
                .context("original invitation key is invalid")?;
            reply_context.source_sender = sender.into();
            reply_context.source_message = message.into();
        }
        let _publication = self.publication.lock().await;
        self.authority
            .publish_receipt(&reply_context, &event)
            .await?;
        Ok(receipt)
    }
}

#[derive(Default)]
pub struct RoutingRuntime {
    service: RwLock<Option<Weak<RoutingService>>>,
}
impl RoutingRuntime {
    pub fn bind(&self, service: &Arc<RoutingService>) -> Result<()> {
        let mut slot = self.service.write().expect("Station routing resource");
        ensure!(
            slot.as_ref().and_then(Weak::upgrade).is_none(),
            "Station routing is already mounted"
        );
        *slot = Some(Arc::downgrade(service));
        Ok(())
    }
    fn service(&self) -> Result<Arc<RoutingService>> {
        self.service
            .read()
            .expect("Station routing resource")
            .as_ref()
            .and_then(Weak::upgrade)
            .context("Session routing is unavailable on this backend")
    }
    pub async fn offer(
        &self,
        context: RoutingContext,
        input: OfferRequest,
    ) -> Result<OfferReceipt> {
        self.service()?.offer(context, input).await
    }
    pub async fn send_confirmed(
        &self,
        context: RoutingContext,
        input: ConfirmedRequest,
    ) -> Result<Receipt> {
        self.service()?.send_confirmed(context, input).await
    }
}


