//! Thin Adam execution adapter. Sample lifecycle and rule policy are Transcreation-owned.
use super::*;
use polar_transcreation::learning::{LearningModel,LearningSample,Replacement};
pub(super) struct AdamLearning {state:Mutex<polar_inhouse_backend_adam::AdamAgentRunner>,owner:String}
impl AdamLearning {
    pub fn new(owner:String,credentials:Arc<dyn polar_inhouse_backend_adam::AdamOAuthCredentialResolver>)->Result<Self>{
        let config=polar_inhouse_backend_adam::parse_adam_agent_config("transcreation-learning",r#"{"llm_connection_mode":"openai","model":"gpt-6-luna","reasoning_effort":"low","max_tool_iterations":1}"#)?;
        Ok(Self{owner,state:Mutex::new(polar_inhouse_backend_adam::AdamAgentRunner::new(config).with_oauth_credential_resolver(credentials))})
    }
}
#[async_trait::async_trait]
impl LearningModel for AdamLearning {
    async fn infer(&self,sample:&LearningSample)->Result<Vec<Replacement>,String>{
        let mut runner=self.state.lock().await;
        let result=polar_transcreation::edit(&mut *runner,polar_transcreation::EditRequest {
            project:sample.refinement.project.clone(),scope:None,operation_id:sample.intent.clone(),base_revision:0,
            original:sample.final_text.clone(),edit:polar_transcreation::EditKind::LearnPreferences{original:sample.refinement.original.clone(),refined:sample.refinement.refined.clone()},
        },polar_transcreation::ExecutionConfig {
            turn_ref:polar_inhouse_contract::InhouseBackendTurnRef{owner_id:self.owner.clone(),thread_id:format!("refine-learning:{}",sample.intent),inhouse_polar_id:"transcreation-learning".into(),request_message_id:sample.intent.clone(),backend_profile_id:"transcreation-learning".into()},
            model:Some("gpt-6-luna".into()),timeout:Duration::from_secs(30),max_text_bytes:64*1024,context_window_tokens:128000,reserve_tokens:16000,
        }).await.map_err(|e|e.to_string())?;
        serde_json::from_str(&result.text).map_err(|e|e.to_string())
    }
}
