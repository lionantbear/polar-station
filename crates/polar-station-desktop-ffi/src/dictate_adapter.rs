//! Concrete IO for the shared Dictate application. No delivery or draft policy.
/// Adapts the IPC client to the existing application ports; owns no policy.
pub struct DictateSpeech(polar_station_client::dictate_speech::DictateSpeech);
impl std::ops::Deref for DictateSpeech {
    type Target = polar_station_client::dictate_speech::DictateSpeech;
    fn deref(&self) -> &Self::Target { &self.0 }
}
impl DictateSpeech {
    pub fn local() -> Result<Self, String> {
        polar_station_client::dictate_speech::DictateSpeech::local().map(Self)
    }
    
}
use polar_station_workspace::dictate::application::{CapturePort, Host, Ports};
use serde_json::Value;
use std::path::Path;

#[derive(Default)]
pub(crate) struct StationDictateIo {
    host: Option<DictateSpeech>,
}

impl Ports for StationDictateIo {
    fn connect_host(&mut self) -> Result<(), String> {
        self.host = Some(DictateSpeech::local()?);
        Ok(())
    }

    fn host(&self) -> Option<&dyn Host> {
        self.host.as_ref().map(|host| host as &dyn Host)
    }

    fn exchange(
        &self,
        path: &Path,
        document: Option<&str>,
        maximum: usize,
    ) -> Result<Option<String>, String> {
        crate::native_state_file::exchange(path, document, maximum)
            .map_err(|error| error.to_string())
    }

    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
}

impl Host for DictateSpeech {
    fn request(&self, payload: Value) -> Result<Value, String> {
        self.0.request(payload)
    }
}

impl CapturePort for DictateSpeech {
    #[allow(clippy::too_many_arguments)] // Preserve the existing capture port while adding trusted project context.
    fn begin(
        &self,
        id: &str,
        context: Option<String>,
        reference: String,
        project:Option<String>,
        cleanup: bool,
        is_test: bool,
    ) -> Result<(), String> {
        self.0.begin(id, context, reference, project, cleanup, is_test)
    }
}
