//! Learning reuses the Host document table, transaction and migration owner.
use crate::{HostDatabase,HostDocument,read_document,write_document};
use polar_transcreation::learning::{LearningStore,LearningState,LearningChange};
#[derive(Clone)]
pub struct SqliteLearningStore(pub HostDatabase);
impl LearningStore for SqliteLearningStore {
    fn load(&self)->Result<LearningState,String>{
        self.0.runtime().map_err(|e|e.to_string())?.read_blocking(|db| {
            Ok(read_document(db,HostDocument::TranscreationPreferences)?.map(|b|serde_json::from_slice(&b)).transpose())
        }).map_err(|e|e.to_string())?.map_err(|e|e.to_string()).map(Option::unwrap_or_default)
    }
    fn change(&self,change:LearningChange)->Result<LearningState,String>{
        self.0.runtime().map_err(|e|e.to_string())?.write_blocking(move |db| {
            let tx=db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let mut state=read_document(&tx,HostDocument::TranscreationPreferences)?.map(|bytes|serde_json::from_slice::<LearningState>(&bytes)).transpose();
            let result=state.as_mut().map_err(|e|e.to_string()).and_then(|s| {
                let s=s.get_or_insert_with(LearningState::default);s.apply(change)?;Ok(s.clone())
            });
            if let Ok(state)=&result {
                let bytes=serde_json::to_vec(state).map_err(|e|rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                write_document(&tx,HostDocument::TranscreationPreferences,&bytes)?;
                tx.commit()?;
            }
            Ok(result)
        }).map_err(|e|e.to_string())?
    }
}

