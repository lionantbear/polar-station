//! Current Session-only document, without layout import or schema conversion.
use polar_station_store_sqlite::{read_document, write_document, HostDatabase, HostDocument};
use polar_tmux_workspace::SessionRuntimeState;
use rusqlite::TransactionBehavior;

#[derive(Debug, Clone)]
pub(crate) struct SessionStore {
    database: HostDatabase,
}

impl SessionStore {
    pub(crate) fn from_database(database: HostDatabase) -> Self {
        Self { database }
    }
    
    pub(crate) fn load_runtime_or_initialize(&self) -> anyhow::Result<SessionRuntimeState> {
        self.database
            .runtime()?
            .write_blocking(|connection| {
                let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let state: SessionRuntimeState = if let Some(bytes) =
                    read_document(&tx, HostDocument::SessionRuntime)?
                {
                    serde_json::from_slice(&bytes)
                        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?
                } else {
                    let state = SessionRuntimeState::new();
                    write_document(
                        &tx,
                        HostDocument::SessionRuntime,
                        &serde_json::to_vec(&state).expect("Session state serializes"),
                    )?;
                    state
                };
                state
                    .validate()
                    .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
                tx.commit()?;
                Ok(state)
            })
            .map_err(Into::into)
    }
    pub(crate) fn save_runtime(&self, state: &SessionRuntimeState) -> anyhow::Result<()> {
        state.validate()?;
        let bytes = serde_json::to_vec(state)?;
        self.database.runtime()?.write_blocking(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            write_document(&tx, HostDocument::SessionRuntime, &bytes)?;
            tx.commit()?;
            Ok(())
        })?;
        Ok(())
    }
}


