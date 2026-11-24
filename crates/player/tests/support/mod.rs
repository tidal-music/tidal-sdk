// Each test crate uses a subset of the shared helpers.
#![allow(dead_code, unused_imports)]

pub mod contracts;
pub mod drm_provider;
pub mod player_events;

#[cfg(not(target_arch = "wasm32"))]
pub mod media;
#[cfg(not(target_arch = "wasm32"))]
pub mod playback;
