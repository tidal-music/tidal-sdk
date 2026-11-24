/// Receives telemetry when a preparation or playback history is dropped.
/// Enqueue synchronously; delivery must not block or panic during destruction.
#[cfg_attr(feature = "uniffi", uniffi::export(with_foreign))]
pub trait TidalEventSender: Send + Sync {
    fn send_event(&self, event: ReportingEvent);
}

#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct ReportingEvent {
    pub group: String,
    pub name: String,
    pub version: u32,
    pub payload: String,
}
