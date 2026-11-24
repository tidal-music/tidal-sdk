use serde::Serialize;

use super::item_event::PlayerItemReportingEvent;
use crate::model::{AudioFormat, Error, MediaProduct};
use crate::platforms::events::ReportingEvent;

pub(crate) trait MaybeFrom<T>: Sized {
    fn maybe_from(value: T) -> Option<Self>;
}

pub(super) trait FindEvent<'a>:
    Iterator<Item = &'a PlayerItemReportingEvent> + Sized
{
    fn find_first_event<T: 'a>(&mut self) -> Option<&'a T>
    where
        &'a T: TryFrom<&'a PlayerItemReportingEvent>,
    {
        self.find_map(|event| event.try_into().ok())
    }
}

impl<'a, I> FindEvent<'a> for I where I: Iterator<Item = &'a PlayerItemReportingEvent> {}

pub(super) fn reporting_event(
    group: &str,
    name: &str,
    payload: impl Serialize,
) -> Result<ReportingEvent, serde_json::Error> {
    Ok(ReportingEvent {
        group: group.into(),
        name: name.into(),
        version: 2,
        payload: serde_json::to_string(&payload)?,
    })
}

pub(super) trait ToTimestamp {
    fn to_timestamp(&self) -> u64;
}

impl ToTimestamp for web_time::SystemTime {
    fn to_timestamp(&self) -> u64 {
        self.duration_since(web_time::SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
}

pub(super) fn audio_mode(product: &MediaProduct, formats: &[AudioFormat]) -> Option<String> {
    match product {
        MediaProduct::Video { .. } => None,
        MediaProduct::Track { .. } if formats.contains(&AudioFormat::Eac3Joc) => {
            Some("DOLBY_ATMOS".into())
        }
        MediaProduct::Track { .. } if !formats.is_empty() => Some("STEREO".into()),
        _ => None,
    }
}

pub(super) fn quality(product: &MediaProduct, formats: &[AudioFormat]) -> String {
    if matches!(product, MediaProduct::Video { .. }) {
        return "VQ N/A".into();
    }
    [
        (AudioFormat::FlacHires, "HI_RES_LOSSLESS"),
        (AudioFormat::Flac, "LOSSLESS"),
        (AudioFormat::AacLc, "HIGH"),
        (AudioFormat::HeAacV1, "LOW"),
    ]
    .into_iter()
    .find_map(|(format, quality)| formats.contains(&format).then_some(quality))
    .unwrap_or("AQ N/A")
    .into()
}

pub(super) fn error_code(error: &Error) -> &str {
    match error {
        Error::Denied { code, .. } | Error::Network { code, .. } | Error::Other { code, .. } => {
            code
        }
    }
}
