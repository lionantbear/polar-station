//! RTCP adaptation for the pinned media dependency; no media clock or scheduler.
use std::sync::Arc;
use webrtc::interceptor::{
    stream_info::StreamInfo, Attributes, Error, Interceptor, InterceptorBuilder, RTCPReader,
    RTCPWriter, RTPReader, RTPWriter,
};
use webrtc::rtcp::{packet::Packet, sender_report::SenderReport};
use webrtc::{
    api::{interceptor_registry::register_default_interceptors, media_engine::MediaEngine},
    interceptor::registry::Registry,
};

pub(super) fn registry(engine: &mut MediaEngine) -> Result<Registry, webrtc::Error> {
    let mut registry = Registry::new();
    // The report generator captures its writer at bind time. Install the guard
    // FIRST so its generated packets pass through it, not just manual writes.
    registry.add(Box::new(InitializedSenderClock));
    register_default_interceptors(registry, engine)
}

// interceptor 0.17.2 generates an SR with RTP time u32::MAX before
// first RTP (last sample time is UNIX_EPOCH). RFC3550 6.4 and Pion's
// sender_stream fix require suppressing those uninitialized sender reports.
// Keep the dependency's scheduling, timestamps and all real media unchanged.
struct InitializedSenderClock;
impl InterceptorBuilder for InitializedSenderClock {
    fn build(&self, _: &str) -> Result<Arc<dyn Interceptor + Send + Sync>, Error> {
        Ok(Arc::new(Self))
    }
}
#[async_trait::async_trait]
impl Interceptor for InitializedSenderClock {
    async fn bind_rtcp_reader(
        &self,
        reader: Arc<dyn RTCPReader + Send + Sync>,
    ) -> Arc<dyn RTCPReader + Send + Sync> {
        reader
    }
    async fn bind_rtcp_writer(
        &self,
        writer: Arc<dyn RTCPWriter + Send + Sync>,
    ) -> Arc<dyn RTCPWriter + Send + Sync> {
        Arc::new(InitializedReports(writer))
    }
    async fn bind_local_stream(
        &self,
        _: &StreamInfo,
        writer: Arc<dyn RTPWriter + Send + Sync>,
    ) -> Arc<dyn RTPWriter + Send + Sync> {
        writer
    }
    async fn unbind_local_stream(&self, _: &StreamInfo) {}
    async fn bind_remote_stream(
        &self,
        _: &StreamInfo,
        reader: Arc<dyn RTPReader + Send + Sync>,
    ) -> Arc<dyn RTPReader + Send + Sync> {
        reader
    }
    async fn unbind_remote_stream(&self, _: &StreamInfo) {}
    async fn close(&self) -> Result<(), Error> {
        Ok(())
    }
}

struct InitializedReports(Arc<dyn RTCPWriter + Send + Sync>);
#[async_trait::async_trait]
impl RTCPWriter for InitializedReports {
    async fn write(
        &self,
        packets: &[Box<dyn Packet + Send + Sync>],
        attributes: &Attributes,
    ) -> Result<usize, Error> {
        let retained: Vec<_> = packets
            .iter()
            .filter(|packet| {
                packet
                    .as_any()
                    .downcast_ref::<SenderReport>()
                    .is_none_or(|sr| sr.packet_count != 0)
            })
            .cloned()
            .collect();
        if retained.is_empty() {
            return Ok(0);
        }
        self.0.write(&retained, attributes).await
    }
}


