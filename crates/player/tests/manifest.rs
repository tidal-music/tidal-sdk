//! Loading media through the manifest the API returns for a product: which request the player
//! sends for its configuration, and how the response reaches the host's media player.
#![cfg(not(target_arch = "wasm32"))]

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use player::{
    AudioFormat, CertificateRequest, DrmInput, Error, HttpMethod, HttpRequest, HttpResponse,
    HttpTransport, ManifestConfiguration, ManifestType, ManifestUsage, MediaInput, MediaItem,
    MediaProduct, PlaybackSeed, Player, TransportError, UriScheme,
};
use serde_json::{json, Value};
use support::media::{self, Media, MediaCall};
use support::playback::{dependencies, player_config};
use support::player_events;
use tokio::sync::Notify;

/// Answers every request with the same response and records what it was asked.
struct Transport {
    requests: Mutex<Vec<HttpRequest>>,
    body: Vec<u8>,
    /// The test releases the response once it has seen the request.
    gate: Option<(Notify, Notify)>,
}

impl Transport {
    fn new(body: Value) -> Arc<Self> {
        Arc::new(Self {
            requests: Mutex::new(Vec::new()),
            body: serde_json::to_vec(&body).unwrap(),
            gate: None,
        })
    }

    fn url(&self, index: usize) -> url::Url {
        url::Url::parse(&self.requests.lock().unwrap()[index].url).unwrap()
    }
}

#[async_trait::async_trait]
impl HttpTransport for Transport {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        self.requests.lock().unwrap().push(request);
        if let Some((started, release)) = &self.gate {
            started.notify_one();
            release.notified().await;
        }
        Ok(HttpResponse {
            status_code: 200,
            body: self.body.clone(),
        })
    }
}

struct Fixture {
    player: Player,
    media: Media,
    /// The item the media player installed last.
    item: Option<Arc<dyn MediaItem>>,
}

impl Fixture {
    fn new(transport: Arc<Transport>) -> Self {
        let (media_factory, media) = media::scripted(media::Script::default());
        let mut dependencies = dependencies(Arc::new(player_events::Ignore), media_factory);
        dependencies.http_transport = transport;
        Self {
            player: Player::new(player_config(), dependencies),
            media,
            item: None,
        }
    }

    /// Loads the product and returns what the media player was asked to render.
    async fn load(&mut self, product: MediaProduct) -> Result<MediaInput, Error> {
        let seed = self.player.prepare(product);
        self.load_seed(seed).await
    }

    async fn load_seed(&mut self, seed: Arc<PlaybackSeed>) -> Result<MediaInput, Error> {
        let loading = self.player.play(seed);
        let item = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::select! {
                result = loading => {
                    result?;
                    unreachable!("loading finishes after the media player installs the item")
                }
                item = installed(&mut self.media) => Ok::<_, Error>(item),
            }
        })
        .await
        .expect("loading timed out")?;
        let input = item.media_input();
        self.item = Some(item);
        Ok(input)
    }

    /// Fetches the installed item's certificate and license, returning their URLs.
    async fn fetch_drm(&self, transport: &Transport) -> (String, String) {
        let item = self.item.as_ref().expect("no installed item");
        let first = transport.requests.lock().unwrap().len();
        item.clone()
            .fetch_certificate(CertificateRequest::Get)
            .await
            .unwrap();
        item.clone()
            .fetch_license(b"challenge".to_vec())
            .await
            .unwrap();
        let requests = transport.requests.lock().unwrap();
        let [certificate, license] = &requests[first..] else {
            panic!("expected a certificate and a license request")
        };
        assert_eq!(certificate.method, HttpMethod::Get);
        assert_eq!(license.method, HttpMethod::Post);
        (certificate.url.clone(), license.url.clone())
    }
}

/// Completes the installation the player asks the media player for, and returns its item.
async fn installed(media: &mut Media) -> Arc<dyn MediaItem> {
    loop {
        if let media::Native::Command(_, MediaCall::SetCurrentAndPlay(call)) =
            media.native.recv().await.unwrap()
        {
            call.reply.complete(Ok(()));
            return call.input;
        }
    }
}

fn track(id: &str) -> MediaProduct {
    MediaProduct::Track { id: id.into() }
}

fn video(id: &str) -> MediaProduct {
    MediaProduct::Video { id: id.into() }
}

fn drm_data(system: &str) -> Value {
    json!({
        "drmSystem": system,
        "licenseUrl": "https://license.example",
        "certificateUrl": "https://certificate.example",
        "initData": [],
    })
}

#[tokio::test]
async fn a_track_manifest_reaches_the_media_player_with_its_formats_normalization_and_drm() {
    let transport = Transport::new(json!({"data": {"id": "resolved-product", "attributes": {
        "uri": "data:application/vnd.apple.mpegurl;base64,dGVzdA==",
        "formats": ["FUTURE_FORMAT", "FLAC"],
        "albumAudioNormalizationData": {"peakAmplitude": 0.5, "replayGain": -3.0},
        "drmData": {
            "drmSystem": "FAIRPLAY",
            "licenseUrl": "https://license.example",
            "certificateUrl": "https://certificate.example",
            "initData": ["init"]
        }
    }}}));
    let mut f = Fixture::new(transport.clone());
    let input = f.load(track("track/id")).await.unwrap();

    assert_eq!(
        input.url,
        "data:application/vnd.apple.mpegurl;base64,dGVzdA=="
    );
    assert_eq!(input.formats, [AudioFormat::Flac]);
    let album = input.album_normalization_data.unwrap();
    assert_eq!(album.peak_amplitude, Some(0.5));
    assert_eq!(album.replay_gain, Some(-3.0));
    assert_eq!(input.track_normalization_data, None);
    assert_eq!(input.drm, Some(DrmInput::OnDemand));
    {
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, HttpMethod::Get);
        assert_eq!(
            url::Url::parse(&requests[0].url).unwrap().path(),
            "/v2/trackManifests/track%2Fid"
        );
    }
    assert_eq!(
        f.fetch_drm(&transport).await,
        (
            "https://certificate.example".into(),
            "https://license.example".into()
        )
    );
}

#[tokio::test]
async fn a_video_manifest_uses_its_link() {
    let transport = Transport::new(json!({"data": {"id": "resolved-product", "attributes": {
        "link": {"href": "https://media.example/video.m3u8"}
    }}}));
    let mut f = Fixture::new(transport.clone());
    let input = f.load(video("video-1")).await.unwrap();

    assert_eq!(input.url, "https://media.example/video.m3u8");
    assert_eq!(input.drm, None);
    assert!(input.formats.is_empty());
    assert_eq!(
        transport.requests.lock().unwrap()[0].url,
        "https://openapi.tidal.com/v2/videoManifests/video-1?uriScheme=DATA&usage=DOWNLOAD"
    );
}

#[tokio::test]
async fn track_requests_carry_the_configured_parameters() {
    let default = ManifestConfiguration::default();
    let mut dash = default.clone();
    dash.track_configuration.manifest_type = ManifestType::MpegDash;
    dash.track_configuration.audio_formats = vec![AudioFormat::FlacHires];
    dash.track_configuration.adaptive = true;
    dash.track_configuration.uri_scheme = UriScheme::Https;
    dash.track_configuration.usage = ManifestUsage::Playback;
    let mut lossy = default.clone();
    lossy.track_configuration.audio_formats = vec![AudioFormat::AacLc, AudioFormat::Flac];
    let mut other = default.clone();
    other.track_configuration.audio_formats = vec![AudioFormat::HeAacV1, AudioFormat::Eac3Joc];

    for (configuration, query) in [
        (
            default,
            "adaptive=false&formats=HEAACV1&manifestType=HLS&uriScheme=DATA&usage=DOWNLOAD",
        ),
        (
            dash,
            "adaptive=true&formats=FLAC_HIRES&manifestType=MPEG_DASH&uriScheme=HTTPS&usage=PLAYBACK",
        ),
        (
            lossy,
            "adaptive=false&formats=AACLC&formats=FLAC&manifestType=HLS&uriScheme=DATA&usage=DOWNLOAD",
        ),
        (
            other,
            "adaptive=false&formats=HEAACV1&formats=EAC3_JOC&manifestType=HLS&uriScheme=DATA&usage=DOWNLOAD",
        ),
    ] {
        let transport = Transport::new(json!({"data": {"id": "t", "attributes": {
            "uri": "https://media.example/track"
        }}}));
        let mut f = Fixture::new(transport.clone());
        f.player.set_manifest_configuration(configuration.clone());
        assert_eq!(f.player.get_manifest_configuration(), configuration);
        f.load(track("1")).await.unwrap();
        assert_eq!(transport.url(0).query().unwrap(), query);
    }
}

#[tokio::test]
async fn track_and_video_requests_use_independent_updated_configuration() {
    let transport = Transport::new(json!({"data": {"id": "resolved-product", "attributes": {
        "uri": "https://media.example/track.mpd",
        "link": {"href": "https://media.example/video.m3u8"},
    }}}));
    let mut f = Fixture::new(transport.clone());
    let mut configuration = f.player.get_manifest_configuration();
    configuration.track_configuration.manifest_type = ManifestType::MpegDash;
    configuration.track_configuration.uri_scheme = UriScheme::Https;
    f.player.set_manifest_configuration(configuration.clone());
    f.load(track("1")).await.unwrap();
    f.load(video("2")).await.unwrap();

    configuration.video_configuration.uri_scheme = UriScheme::Https;
    configuration.video_configuration.usage = ManifestUsage::Playback;
    f.player.set_manifest_configuration(configuration);
    f.load(video("3")).await.unwrap();
    f.load(track("4")).await.unwrap();

    let query = |index| transport.url(index).query().unwrap().to_string();
    assert!(query(0).contains("manifestType=MPEG_DASH") && query(0).contains("uriScheme=HTTPS"));
    assert_eq!(query(1), "uriScheme=DATA&usage=DOWNLOAD");
    assert_eq!(query(2), "uriScheme=HTTPS&usage=PLAYBACK");
    assert_eq!(query(3), query(0));
}

#[tokio::test]
async fn changing_the_configuration_does_not_change_a_request_in_flight() {
    let transport = Arc::new(Transport {
        requests: Mutex::new(Vec::new()),
        body: serde_json::to_vec(&json!({"data": {"id": "resolved-product", "attributes": {
            "uri": "https://media.example/track.mpd",
            "drmData": drm_data("WIDEVINE"),
        }}}))
        .unwrap(),
        gate: Some((Notify::new(), Notify::new())),
    });
    let mut configuration = ManifestConfiguration::default();
    configuration.track_configuration.manifest_type = ManifestType::MpegDash;
    let mut f = Fixture::new(transport.clone());
    f.player.set_manifest_configuration(configuration);

    let seed = f.player.prepare(track("1"));
    let (started, release) = transport.gate.as_ref().unwrap();
    started.notified().await;
    f.player
        .set_manifest_configuration(ManifestConfiguration::default());
    release.notify_one();
    let input = f.load_seed(seed).await.unwrap();

    assert_eq!(input.drm, Some(DrmInput::OnDemand));
    let requests = transport.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].url.contains("manifestType=MPEG_DASH"));
}

#[tokio::test]
async fn drm_info_comes_from_the_response_for_tracks_and_videos() {
    for manifest_type in [ManifestType::Hls, ManifestType::MpegDash] {
        for system in ["FAIRPLAY", "WIDEVINE"] {
            let drm = json!({"drmSystem": system, "licenseUrl": "https://license.example", "certificateUrl": "https://certificate.example", "initData": ["first", "second"]});
            for product in [track("1"), video("2")] {
                let transport =
                    Transport::new(json!({"data": {"id": "resolved-product", "attributes": {
                        "uri": "https://media.example/track",
                        "link": {"href": "https://media.example/video"},
                        "drmData": drm,
                    }}}));
                let mut f = Fixture::new(transport.clone());
                let mut configuration = f.player.get_manifest_configuration();
                configuration.track_configuration.manifest_type = manifest_type;
                f.player.set_manifest_configuration(configuration);

                let input = f.load(product).await.unwrap();
                assert_eq!(input.drm, Some(DrmInput::OnDemand));
                assert_eq!(
                    f.fetch_drm(&transport).await,
                    (
                        "https://certificate.example".into(),
                        "https://license.example".into()
                    )
                );
            }
        }
    }
}

#[tokio::test]
async fn content_without_drm_data_has_no_drm_input() {
    for product in [track("1"), video("2")] {
        let transport = Transport::new(json!({"data": {"id": "resolved-product", "attributes": {
            "uri": "https://media.example/track",
            "link": {"href": "https://media.example/video"},
            "drmData": null,
        }}}));
        let input = Fixture::new(transport).load(product).await.unwrap();
        assert_eq!(input.drm, None);
    }
}

/// The code that tells a failure to read the manifest from a failure to reach the API.
fn manifest_error_code(error: Error) -> String {
    match error {
        Error::Other {
            code, can_retry, ..
        } => {
            assert!(!can_retry);
            code
        }
        error => panic!("expected an other error: {error:?}"),
    }
}

#[tokio::test]
async fn manifests_the_player_cannot_read_fail_to_load() {
    let complete = drm_data("WIDEVINE");
    let mut cases = vec![(
        "drm system the player does not know",
        drm_data("FUTURE_DRM"),
    )];
    for field in ["drmSystem", "licenseUrl", "certificateUrl"] {
        let mut missing = complete.clone();
        missing.as_object_mut().unwrap().remove(field);
        cases.push(("missing required drm field", missing));
        let mut null = complete.clone();
        null[field] = Value::Null;
        cases.push(("null required drm field", null));
    }
    for init_data in [json!("init"), json!([null]), json!([42])] {
        let mut malformed = complete.clone();
        malformed["initData"] = init_data;
        cases.push(("malformed init data", malformed));
    }

    for (case, drm) in cases {
        for product in [track("1"), video("2")] {
            let transport =
                Transport::new(json!({"data": {"id": "resolved-product", "attributes": {
                    "uri": "https://media.example/track",
                    "link": {"href": "https://media.example/video"},
                    "drmData": drm,
                }}}));
            let error = Fixture::new(transport)
                .load(product)
                .await
                .err()
                .expect("loading fails");
            let expected = if case.starts_with("drm system") {
                "muds"
            } else {
                "mdec"
            };
            assert_eq!(manifest_error_code(error), expected, "{case}: {drm}");
        }
    }
}

#[tokio::test]
async fn responses_without_a_manifest_url_are_not_content_not_found() {
    for body in [
        json!({"data": null}),
        json!({"data": {"id": "resolved-product"}}),
        json!({"data": {"id": "resolved-product", "attributes": {}}}),
        json!({"data": {"id": "resolved-product", "attributes": {"uri": null, "link": {"href": null}}}}),
    ] {
        for product in [track("1"), video("2")] {
            let error = Fixture::new(Transport::new(body.clone()))
                .load(product)
                .await
                .err()
                .expect("loading fails");
            assert_eq!(manifest_error_code(error), "mmmu", "{body}");
        }
    }
    for product in [track("1"), video("2")] {
        let error = Fixture::new(Transport::new(json!({"data": 42})))
            .load(product)
            .await
            .err()
            .expect("loading fails");
        assert_eq!(manifest_error_code(error), "mdec");
    }
}
