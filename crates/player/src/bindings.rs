use std::sync::Arc;

use crate::model::PlayerConfig;
use crate::platforms::media::MediaPlayerFactory;
use crate::platforms::player_events::PlayerEvents;
use crate::platforms::{
    authenticator, drm, media, transport, Authenticator, DrmProvider, HttpTransport,
    TidalEventSender,
};
use crate::player::{Player, PlayerDependencies};

#[uniffi::export]
impl Player {
    #[uniffi::constructor(name = "new")]
    #[allow(clippy::too_many_arguments)]
    pub fn from_platforms(
        config: PlayerConfig,
        transport: Arc<dyn HttpTransport>,
        authenticator: Arc<dyn Authenticator>,
        media_factory: Arc<dyn MediaPlayerFactory>,
        drm_provider: Arc<dyn DrmProvider>,
        event_sender: Arc<dyn TidalEventSender>,
        events: Arc<dyn PlayerEvents>,
    ) -> Self {
        with_runtime(|| {
            Self::new(
                config,
                PlayerDependencies {
                    player_events: events,
                    media_factory,
                    http_transport: transport,
                    authenticator,
                    drm_provider,
                    event_sender,
                },
            )
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn with_runtime<T>(construct: impl FnOnce() -> T) -> T {
    use std::future::Future;
    use std::task::{Context, Poll, Waker};

    // Construction is synchronous, so this future completes in its first poll.
    let mut construction = std::pin::pin!(uniffi::deps::async_compat::Compat::new(async {
        construct()
    }));
    match construction
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(player) => player,
        Poll::Pending => unreachable!("player construction does not await"),
    }
}

#[cfg(target_arch = "wasm32")]
fn with_runtime<T>(construct: impl FnOnce() -> T) -> T {
    construct()
}

impl From<uniffi::UnexpectedUniFFICallbackError> for transport::Error {
    fn from(error: uniffi::UnexpectedUniFFICallbackError) -> Self {
        Self::OtherError {
            description: error.to_string(),
        }
    }
}

impl From<uniffi::UnexpectedUniFFICallbackError> for authenticator::Error {
    fn from(error: uniffi::UnexpectedUniFFICallbackError) -> Self {
        Self::AuthenticationFailed {
            description: error.to_string(),
        }
    }
}

impl From<uniffi::UnexpectedUniFFICallbackError> for drm::Error {
    fn from(error: uniffi::UnexpectedUniFFICallbackError) -> Self {
        Self::Failed {
            description: error.to_string(),
            code: None,
        }
    }
}

impl From<uniffi::UnexpectedUniFFICallbackError> for media::Error {
    fn from(error: uniffi::UnexpectedUniFFICallbackError) -> Self {
        Self::Other {
            code: "CALLBACK_ERROR".into(),
            description: error.to_string(),
        }
    }
}

// web-time uses its own SystemTime on Wasm; retain UniFFI's standard timestamp wire format.
#[cfg(target_arch = "wasm32")]
mod timestamps {
    use crate::UniFfiTag;
    use uniffi::{FfiConverter, MetadataBuffer};
    use web_time::SystemTime;

    type NativeTime = std::time::SystemTime;

    // SAFETY: all serialisation and metadata delegate to UniFFI's timestamp converter.
    unsafe impl FfiConverter<UniFfiTag> for SystemTime {
        uniffi::ffi_converter_rust_buffer_lift_and_lower!(UniFfiTag);

        fn write(time: Self, buffer: &mut Vec<u8>) {
            let time = match time.duration_since(SystemTime::UNIX_EPOCH) {
                Ok(duration) => NativeTime::UNIX_EPOCH + duration,
                Err(error) => NativeTime::UNIX_EPOCH - error.duration(),
            };
            <NativeTime as FfiConverter<UniFfiTag>>::write(time, buffer);
        }

        fn try_read(buffer: &mut &[u8]) -> uniffi::Result<Self> {
            let time = <NativeTime as FfiConverter<UniFfiTag>>::try_read(buffer)?;
            Ok(match time.duration_since(NativeTime::UNIX_EPOCH) {
                Ok(duration) => SystemTime::UNIX_EPOCH + duration,
                Err(error) => SystemTime::UNIX_EPOCH - error.duration(),
            })
        }

        const TYPE_ID_META: MetadataBuffer = <NativeTime as FfiConverter<UniFfiTag>>::TYPE_ID_META;
    }

    uniffi::derive_ffi_traits!(local SystemTime);
}
