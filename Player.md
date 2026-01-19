# Player

The Player is a module that encapsulates functionality for playback and offlining of TIDAL media products. The
Player module constitutes the only recommended way that an app can supply this functionality.

Player consists of two main parts; the Playback Engine (PE) and the Offline Engine (OE).

The PE is responsible for playback of media products, both online streaming and offlined ones.

* Its API is designed to be small and easy to use, but still allow for optimal playback performance.
* Two core concepts of the PE are “playback state” and “active media product” describing what the PE is currently
  doing (
  playback state) and to which media product it is doing it (active media product).

The OE is responsible for downloading, storing and indexing media products.

* Offlining is the act of downloading a media product to local permanent storage so that it can later be accessed and
  played, even when there is no internet connection available.
* End users will typically use this functionality to be able to play media products even when there is no internet
  connection available.
* Offlining functionality can also be used by users who want to save network bandwidth.

The PE and OE could have been formalized as two separate modules. They are, however, closely related and there exist
several good arguments for encapsulating both these areas of functionality into one module:

* Having an OE makes no sense unless you also have a PE
* Both the PE and OE rely on many of the same types
* Both the PE and the OE use the same content. Having them in one module enables the Player to have full control over
  all that content (optimize cache handling).

In order to illustrate the separation between these two parts, this design is broken up into these two parts.

# Terminology

* _Playback Engine (PE)_ - The part of the Player responsible for playback of media products.
* _Offline Engine (OE)_ - The part of the Player responsible for downloading, storing and indexing media products.
* _End user_ - The user of the TIDAL music streaming app.
* _Media product_ - A TIDAL product of type track or video.
* _Active media product_ - The media product that the PE is currently working on. It is the media product that the PE is
  playing, is trying to play or will try to play when play() is called.
* _Media product transition_ - Occurs in the PE when the active media product changes. There are two types of
  transitions:
    * _Explicit transition_ - Occurs whenever a seemingly random media product is requested. Typically when the user
      presses something that shall result in a new media product being played.
    * _Implicit transition_ - Occurs when one media product has finished playing and the next in queue is automatically
      played. Allows the Player to make seamless, gapless, transitions.
* _Play_ - The action of demuxing, decoding and rendering the content for a media product. This action results in
  audible audio and/or visible video.
* _Offline_ - The action of downloading, indexing, validating and storing relevant parts of a media product (
  playbackinfo, manifest, DRM license and content) to permanent (offline) storage on the device.
    * _Offline_ is also used to refer to an offlined media product.
* _Manifest_ - A piece of metadata describing the content, where it can be fetched from etc. E.g. MPEG-DASH, HLS, EMU,
  BTS.
* _Content_ - The actual media files (CMAF, mp4, FLAC etc).

# API

## Player

The `Player` role exposes initialization and configuration.

### init

```java
/**
 * Initializes the Player. Also initializes the PE and an OE.
 *
 * @param credentialsProvider A credentials provider, used by the Player to get access token.
 * @param eventSender An event sender.
 */
init(Auth.CredentialsProvider credentialsProvider,
     EventProducer.EventSender eventSender);
```

### isOfflineMode

```java
/**
 * Property used by the Player (both PE and OE) to know if the app is in online or offline mode.
 * Must be updated by the app each time the app changes offline mode.
 */
Bool isOfflineMode;
```

## PlaybackEngineConfig

The `PlaybackEngineConfig` role exposes configuration for the PE.

### loudnessNormalizationMode

```java
/**
 * The configured loudness normalization mode.
 * Persisted by the Player module.
 */
LoudnessNormalizationMode loudnessNormalizationMode;
```

### loudnessNormalizationPreAmp

```java
/**
 * The configured pre amplification value to be used for loudness normalization. 
 * The value should in general be 4, except for AndroidTv, AppleTv and AirPlay where it should be 0.
 * Persisted by the Player module.
 */
Int loudnessNormalizationPreAmp;
```

### streamingWifiAudioQuality

```java
/**
 * Configured AudioQuality to use when streaming over WiFi.
 * Persisted by the Player module.
 */
AudioQuality streamingWifiAudioQuality;
```

### streamingCellularAudioQuality

```java
/**
 * Configured AudioQuality to use when streaming over cellular.
 * Persisted by the Player module.
 */
AudioQuality streamingCellularAudioQuality;
```

## PlaybackEngine

The `PlaybackEngine` role exposes functionality of the PE.

### load

```java
/**
 * Will reset PE from any current state and immediately make a transition to the selected media product.
 * Playback state will immediately change to NOT_PLAYING.
 * Progress can be tracked via the MediaProductTransitionMessage and PlaybackStateChangedMessage.
 * Async (will return immediately) and thread safe
 *
 * @param mediaProduct The media product to load
 */
Void load(MediaProduct mediaProduct);
```

### setNext

```java
/**
 * Tell PE to make an implicit transition to the selected media product once the currently active finishes playing. MUST be called EVERY TIME next up in playqueue changes.
 * This action does not explicitly effect playback state.
 * Calls to this function when current state is IDLE are ignored.
 * Async (will return immediately) and thread safe. Progress can be tracked via the MediaProductTransition message.
 *
 * @param mediaProduct The media product to set as next
 */
Void next(MediaProduct mediaProduct);
```

### play

```java
/**
 * Start playback of active media product.
 * Target playback state for this action is PLAYING.
 * Calls to this function when current state is IDLE are ignored.
 * Async (will return immediately) and thread safe. Progress can be tracked via the PlaybackStateChange message.
 */
Void play();
```

### pause

```java
/**
 * Pause playback of active media product.
 * Target playback state for this action is NOT_PLAYING.
 * Calls to this function when current state is IDLE are ignored.
 * Async (will return immediately) and thread safe. Progress can be tracked via the PlaybackStateChange message.
 */
Void pause();
```

### seek

```java
/**
 * Change asset position in active media product.
 * This action does not explicitly affect playback state.
 * Calls to this function when current state is IDLE are ignored.
 * Thread safe.
 *
 * @param position The new position to move to in active media product
 */
Void seek(Float position);
```

### skipToNext

```java
/**
 * Change the playing asset to that currently set as next.
 * This action does not explicitly affect playback state.
 * Calls to this function when current state is IDLE are ignored.
 * Thread safe.
 */
Void skipToNext();
```

### reset

```java
/**
 * Reset the PE to initial state i.e. stop playback, removes any media product set as next etc.
 * Playback state will immediately change to IDLE.
 * Async (will return immediately) and thread safe. Progress can be tracked via the PlaybackStateChange message.
 */
Void reset();
```

### mediaProduct

```java
/**
 * Read only property of the currently active media product.
 * This is the same value as was included in the last fired MediaProductTransition message, or null.
 */
MediaProduct mediaProduct;
```

### playbackContext

```java
/**
 * Read only property of the currently active playback context.
 * This is the same value as was included in the last fired MediaProductTransition message, or null.
 */
PlaybackContext playbackContext;
```

### playbackState

```java
/**
 * Read only property of the currently active playback state.
 * This is the same value as was included in the last fired PlaybackStateChangedMessage, or null.
 */
PlaybackState playbackState;
```

### assetPosition

```java
/**
 * Read only property of the current asset position, in seconds, in the media product being played, or null.
 */
Float assetPosition;
```

### bus

```java
/**
 * The bus which the PE uses for all messages and errors fired or raised "asynchronously".
 */
Bus<
        UnexpectedError,
        ContentNotAvailableInLocationError,
        ContentNotAvailableForSubscriptionError,
        MonthlyStreamQuotaExceededError,
        NotAllowedError,
        Common.RetryableError,
        Common.NetworkError,
        MediaProductTransitionMessage,
        PlaybackStateChangedMessage,
        PlaybackQualityChangedMessage,
        StreamingPrivilegesRevokedMessage> bus;
```

## OfflineEngineConfig

The `OfflineEngineConfig` role exposes configuration for the OE.

### offlineAudioQuality

```java
/**
 * Configured AudioQuality used for offlining.
 * Persisted by the Player module.
 */
AudioQuality offlineAudioQuality;
```

### offlineVideoQuality

```java
/**
 * Configured VideoQuality used for offlining.
 * Persisted by the Player module.
 */
VideoQuality offlineVideoQuality;
```

### offlineOverCellular

```java
/**
 * Configures whether offlining should be allowed over cellular network.
 * Persisted by the Player module.
 */
Bool offlineOverCellular;
```

## OfflineEngine

The `OfflineEngine` role exposes functionality of the OE.

### offline

```java
/**
 * Offlines a media product at first convenient time, using configured settings.
 * Async and thread safe. If returns true, progress can be tracked via OfflineStartedMessage, OfflineProgressMessage, OfflineDoneMessage and OfflineFailedMessage.
 *
 * @param mediaProduct The media product to offline
 * @return True if an offline job is created, false otherwise.
 */
Bool offline(MediaProduct mediaProduct);
```

### deleteOffline

```java
/**
 * Deletes an offlined media product. No difference is made between queued, executing or done offlines. Everything is removed for the media product.
 * Async and thread safe. If returns true, progress can be tracked via OfflineDeletedMessage.
 *
 * @param mediaProduct Media product to delete offline for
 * @return True if a delete job is created, False otherwise.
 */
Bool deleteOffline(MediaProduct mediaProduct);
```

### deleteAllOfflines

```java
/**
 * All offlined media products will be deleted. All queued, executing and done offlines will be deleted.
 * Async and thread safe. If returns true, progress can be tracked via AllOfflinesDeletedMessage.
 *
 * @return True if a delete all offlines job is created, False otherwise.
 */
Bool deleteAllOfflines();
```

### getOfflineState

```java
/**
 * Returns offline state of a media product.
 * Thread safe.
 *
 * @param mediaProduct Media product to gett offline state for.
 * @return The state mediaProduct is in.
 */
OfflineState getOfflineState(MediaProduct mediaProduct);
```

### bus

```java
/**
 * The bus which OE uses for all messages and errors fired or raised "asynchronously".
 */
Bus<
        UnexpectedError,
        ContentNotAvailableInLocationError,
        ContentNotAvailableForSubscriptionError,
        OffliningNotAllowedOnDeviceError,
        NotAllowedError,
        Common.RetryableError,
        Common.NetworkError,
        OfflineStartedMessage,
        OfflineProgressMessage,
        OfflineDoneMessage,
        OfflineDeletedMessage,
        AllOfflinesDeletedMessage> bus;
```

## Types

### ProductType

```java
/**
 * Describes the type of media product.
 */
enum ProductType {

    /**
     * The media product is a TIDAL track
     */
    TRACK,

    /**
     * The media product is a TIDAL video
     */
    VIDEO
}
```

### StreamType

```java
/**
 * The StreamType enumeration describes if content is of type live stream or on demand.
 */
enum StreamType {

    /**
     * On demand content.
     */
    ON_DEMAND,

    /**
     * Live content (live stream).
     */
    LIVE
}
```

### AssetPresentation

```java
/**
 * Describes if content represents the full version of a media product, or a preview.
 */
enum AssetPresentation {

    /**
     * The full version
     */
    FULL,

    /**
     * The preview version
     */
    PREVIEW
}
```

### AudioMode

```java
/**
 * Describes the mode of an audio stream.
 */
enum AudioMode {

    /**
     * The audio stream is stereo.
     */
    STEREO,

    /**
     * The audio stream is Sony 360 Reality Audio.
     */
    SONY_360RA,

    /**
     * The audio stream is Dolby Atmos.
     */
    DOLBY_ATMOS
}
```

### AudioQuality

```java
/**
 * Describes the quality of an audio stream. It should be seen as a conceptual quality that does not have a strict technical definition.
 */
enum AudioQuality {

    /**
     * The audio stream is of low quality.
     * (E.g. 96kbps He-AAC)
     */
    LOW,

    /**
     * The audio stream is of high quality.
     * (E.g. 320kbps AAC-LC)
     */
    HIGH,

    /**
     * The audio stream is of lossless quality. 
     * (E.g. CD quality, FLAC 44.1/16)
     */
    LOSSLESS,

    /**
     * @deprecated use HI_RES_LOSSLESS instead
     */
    HI_RES,

    /**
     * The audio stream is of high resolution quality.
     * (E.g. > CD quality)
     */
    HI_RES_LOSSLESS
}
```

### VideoQuality

```java
/**
 * Describes the quality of a video stream. It should be seen as a conceptual quality that does not have a strict technical definition.
 */
enum VideoQuality {

    /**
     * The video stream is of low quality.
     */
    LOW,

    /**
     * The video stream is of medium quality.
     */
    MEDIUM,

    /**
     * The video stream is of high quality.
     */
    HIGH
}
```

### MediaProduct

```java
/**
 * Contains information about a TIDAL media product.
 */
class MediaProduct {

    /**
     * Type of media product 
     */
    ProductType productType;

    /**
     * Product id
     */
    String productId;

    /**
     * Must be set to a unique value for each MediaProduct instance. Used for future referencing of a unique MediaProduct instance in messages.
     */
    String referenceId;

    /**
     * Identifies the UI source type where the media product was requested from.
     * Passed through to corresponding PlayLog event
     */
    String sourceType;

    /**
     * Identifies the UI source id where the media product was requested from.
     * Passed through to corresponding PlayLog event
     */
    String sourceId;
    
    /**
     * Optional extra attributes of the media product. 
     * Passed through to corresponding PlayLog event
     */
    Extras extras;
}
```

### UnexpectedError

```java
/**
 * An unexpected error occurred within the Player. This is a “catch-all” type error, typically indicating an unexpected, internal error in the Player.
 */
class UnexpectedError {
}
```

### ContentNotAvailableInLocationError

```java
/**
 * Error indicating that requested content is not available in the user's location. Caused by playbackinfo subStatus codes 4032 and 4035.
 */
class ContentNotAvailableInLocationError {
}
```

### ContentNotAvailableForSubscriptionError

```java
/**
 * Error indicating that subscription is not eligible for any content, typically used for up-sale messages. Caused by playbackinfo subStatus code 4033.
 */
class ContentNotAvailableForSubscriptionError {
}
```

### MonthlyStreamQuotaExceededError

```java
/**
 * Error indicating that user has exceeded the maximum number of monthly streams defined by his/hers subscription. Caused by playbackinfo subStatus code 4010.
 */
class MonthlyStreamQuotaExceededError {
}
```

### NotAllowedError

```java
/**
 * Error used for all playbackinfo 4xx errors not otherwise specified in this specification, or for other operations that are not allowed but not specifically documented elsewhere. Note that authentication errors are handled by the the Player authentication mechanism. However, if that also fails, this error shall also be used.
 */
class NotAllowedError {
}
```

### OffliningNotAllowedOnDeviceError

```java
/**
 * Error used in cases where the user tries to offline a media product on a device that is not authorised for offlining. Caused by playbackinfo subStatus code 4007.
 */
class OffliningNotAllowedOnDeviceError {
}
```

### PlaybackState

```java
/**
 * States that PE can be in.
 */
enum PlaybackState {

    /**
     * PE has no active media product and nothing set as next. This is the “default” state, nothing is happening and nothing is scheduled to happen. 
     * This is the state which PE boots into, it is also the state reached after the last requested media product finishes playing or after call to reset()
     */
    IDLE,

    /**
     * PE is currently playing the active media product.
     */
    PLAYING,

    /**
     * PE has an active media product, but it is currently not trying to play it. This can be due to the user having pressed pause, or other similar scenarios.
     */
    NOT_PLAYING,

    /**
     * Playback is currently stalled due to some unexpected/unwanted reason but PE is still trying to play.
     E.g. buffering, device resource issues etc.
     * If PE manages to resume playback by itself, state will change to PLAYING. If PE fails to resume playback (error) state will change to IDLE and an error will be raised.
     */
    STALLED
}
```

### LoudnessNormalization

```java
/**
 * Different modes of loudness normalization that the Player supports.
 */
enum LoudnessNormalization {

    /**
     * No loudness normalization shall be applied.
     */
    OFF,

    /**
     * Track style loudness normalization. All tracks get same loudness.
     */
    TRACK,

    /**
     * Album style loudness normalization. All albums get same loudness, tracks of an album are played at the same relative level as on the original release.
     */
    ALBUM
}
```

### PlaybackContext

```java
/**
 * Contains playback related information for active media product.
 */
class PlaybackContext {

    /**
     * The product id of the media product being played. Might differ from requested in case of a replacement.
     */
    String productId;

    /**
     * The stream type of the media product being played. Might differ from requested in case of a replacement.
     */
    StreamType streamType;

    /**
     * The asset presentation of the media product being played. Might differ from requested in case of a replacement.
     */
    AssetPresentation assetPresentation;

    /**
     * The audio mode of the media product being played. Might differ from requested in case of a replacement.
     */
    AudioMode audioMode;

    /**
     * The audio quality of the media product being played. Might differ from requested in case of a replacement.
     */
    AudioQuality audioQuality;

    /**
     * The bit rate indicated in number of bits per second of the media product being played.
     */
    Int audioBitRate;

    /**
     * The bit depth indicated in number of bits used per sample of the media product being played.
     */
    Int audioBitDepth;

    /**
     * The codec name of the media product being played.
     */
    String audioCodec;

    /**
     * The sample rate indicated in Hz of the media product being played.
     */
    Int audioSampleRate;

    /**
     * The video quality of the media product being played. Might differ from requested in case of a replacement.
     */
    VideoQuality videoQuality;

    /**
     * The duration, in seconds, of the media product being played. Might differ from the meta data connected to the requested in case of a replacement.
     */
    Float duration;

    /**
     * The starting position of playback, in seconds, in the current media product. Not mandatory to include.
     */
    Float startAssetPosition;

    /**
     * The playback session id that is used for PlayLog and StreamingMetrics (streamingSessionId) for the currently active playback.
     */
    String playbackSessionId;

    /**
     * Loopback of the referenceId field in the media product that this playback context relates to.
     */
    String referenceId;
}
```

### MediaProductTransitionMessage

```java
/**
 * Message fired when the PE makes a transition to a new active media product. Implicit media product transitions can be assumed to occur exactly when this message is sent.
 */
class MediaProductTransitionMessage {

    /**
     * Loopback of the requested MediaProduct that just finished playing.
     *
     * null if the playback engine just started playing.
     */
    MediaProduct previousMediaProduct;

    /**
     * Loopback of the requested MediaProduct that just became the active one.
     *
     * null if the playback engine just stopped playing (no media product set as next when the currently active ended).
     */
    MediaProduct activeMediaProduct;

    /**
     * Information about the active playback session.
     *
     * null if the playback engine just stopped playing (no media product set as next when the currently active ended).
     */
    PlaybackContext playbackContext;
}
```

### PlaybackStateChangedMessage

```java
/**
 * Message fired when the PE has changed its playback state.
 */
class PlaybackStateChangedMessage {

    /**
     * The playback state in PE.
     */
    PlaybackState playbackState;
}
```

### PlaybackQualityChangedMessage

```java
/**
 * Message fired when playback quality changes during ABR (Adaptive Bitrate) streaming.
 * This occurs when the adaptive bitrate switches to a different quality variant mid-playback.
 */
class PlaybackQualityChangedMessage {

    /**
     * Information about the active playback session.
     *
     * Will contain the changed fields (like: quality, codec, sample rate, sample depth, bit rate).
     */
    PlaybackContext playbackContext;
}
```

### StreamingPrivilegesRevokedMessage

```java
/**
 * Fired whenever this instance of the Player had its streaming privileges revoked.
 */
class StreamingPrivilegesRevokedMessage {

    /**
     * Name of the other device that is currently being used, causing streaming priviliges to be revoked for this instance of the Player
     */
    String otherDevice;
}

```

### OfflineState

```java
/**
 * States that an offlined media product can be in.
 */
enum OfflineState {

    /**
     * The media product is not offlined.
     */
    NOT_OFFLINED,

    /**
     * The media product is offlined, and has passed validation, meaning it can be played.
     */
    OFFLINED_AND_VALID,

    /**
     * The media product is offlined, but it has failed validation, meaning it cannot be played.
     */
    OFFLINED_BUT_NOT_VALID
}
```

### OfflineStartedMessage

```java
/**
 * Message fired when offlining of a media product starts. Fired both for user initiated offlines and potential re-validations.
 */
class OfflineStartedMessage {

    /**
     * Loopback of the referenceId field in the media product that was requested for offlining.
     */
    String referenceId;
}
```

### OfflineProgressMessage

```java
/**
 * Message fired periodically when offlining of a media product is executing. The progress is indicated in percent. Fired both for user initiated offlines and potential re-validations.
 */
class OfflineProgressMessage {

    /**
     * Loopback of the referenceId field in the media product that was requested for offlining.
     */
    String referenceId;

    /**
     * Progress of offlining, in percent.
     */
    Int downloadPercent;
}
```

### OfflineDoneMessage

```java
/**
 * Message fired when offlining of a media product has successfully finished. Fired both for user initiated offlines and potential re-validations.
 */
class OfflineDoneMessage {

    /**
     * Loopback of the referenceId field in the media product that was requested for offlining.
     */
    String referenceId;
}
```

### OfflineDeletedMessage

```java
/**
 * Message fired when an offlined media product has been deleted. Fired both for queued and previously offlined media products.
 */
class OfflineDeletedMessage {

    /**
     * Loopback of the referenceId field in the media product that was requested for offlining.
     */
    String referenceId;
}
```

### AllOfflinesDeletedMessage

```java
/**
 * Message fired when all offlined media products have been deleted.
 */
class AllOfflinesDeletedMessage {
}
```

# Common Functionality and Requirements

## Metrics and Events

The Player must implement _Streaming Metrics_, _PlayLog_ and _Progress_ events for tracking playback and offlining. See
respective event documentation for further information.

## Streaming privileges

The Player must implement Streaming Privileges support, a solution that guarantees a user cannot play on multiple
devices simultaneously, using the same subscription. See streaming privileges documentation for further information.

Whenever the Player receives a message that streaming privileges have been revoked, it shall pause playback and fire a
`StreamingPrivilegesRevokedMessage`.

## HTTP error handling

All information needed by the Player in order to download or stream a media product is received (directly or
indirectly) in the playbackinfo response. There can be multiple subsequent requests made by the Player using
information in that playbackinfo e.g. manifest, content and DRM license, but all these are derived from the initial
playbackinfo response. These resources derived from a playbackinfo response are called sub-resources. The core concept
here is that we handle all errors the same, regardless of which item is being fetched.

* For 5XX responses and 429 responses, the Player shall retry up to 3 times, (making a total of max 4 identical
  requests) using exponential backoff with delays of 500ms, 1000ms, 2000ms. Delays are calculated from the time when a
  5XX response is received or when another network error occurred.
    * When offlining media products, it is more important that stuff gets downloaded sooner or later than giving the
      user quick feedback of issues. Therefore we have a longer period where we retry. Same retry logic (exp backoff
      500,1000,2000ms etc) but up to 9 retries (making a total of 10 identical requests)
* For 4XX responses (except 429), the Player shall never retry the same request.
    * In case a 4XX is received when requesting the playbackinfo an action appropriate to the corresponding error code
      shall be taken (See Streaming API documentation for further information.)
    * In case a 4XX is received when requesting a sub-resource to a playbackinfo response, the entire flow shall be
      restarted from scratch with a new playbackinfo fetch (handbrake).
    * If a handbrake flow is activated, streaming session and playback session shall be ended and new ones started for
      the new flow.
* Network errors, including timeouts shall be retried indefinitely.
* In case max number of 5XX/429 retries have been made:
    * The Player shall raise an error with corresponding error type.

## HTTP Caching

In order to reduce the data usage by the client, and to reduce load on our backend, it is recommended that the Player
caches HTTP responses (playbackinfo and all sub-resources derived from the playbackinfo).

* Caches must correctly respect cache-control headers.
* Caches for manifests and content must only use the path part of the URL as cache key. Reason being that we use
  signed urls, where the signature is unique and part of the query string.

## Media formats

A Player implementation should strive for supporting playback and offlining of fMP4/CMAF files, encrypted using
CENC-CBCS and delivered using DASH or HLS manifests. In addition, the Player should support either Widevine of FairPlay
DRM for decrypting the content.

## Replacement handling

A replacement means that the streaming platform backend returns a media product different from the requested one. This
can mean a different product id, a different audio/video quality, a different asset presentation etc. Replacement
functionality enables us to deliver a very similar media product, in case the one requested was not available. TIDAL
Player needs to be aware of, and correctly handle, replacements in order to correctly signal to the app, PlayLog etc
what is actually being played/offlined.

The Player must always respect and reflect replacements, both for streaming and offlining. In general it means keeping
track of what was requested in a playbackinfo call, and what was actually received.
The OE must always keep track of the requested metadata, since that shall be used by re-validations.
When playing a media product, either by streaming or from offline storage, the actual metadata shall be presented to the
player UI for presentation and the Play log event shall use the actual metadata (in addition to some requested).

# PE Functionality and Requirements

## PlaybackState logic

The PE shall always be in exactly one playback state.

The state of the PE only says something about what the PE is doing right now, not which media product it is doing it to.

Playback state switches shall be indicated by a `PlaybackStateChangeMessage`. See the API description for further
information regarding the various playback states.

## Active media product logic

The active media product defines which media product the PE is currently playing, trying to play, or will play when
`play()` is called. There are two ways for the user of the Player to change the active media product.

1. Explicit transition by calling `load()`. This will immediately change the active media product to the selected one.
1. Implicit transition by calling `next()`. This will change the active media product as soon as the currently active
   media product has finished playing.

When the PE is in state `IDLE`, it has no active media product.

Information about active media products and transitions shall be supplied using the `MediaProductTransitionMessage`.

## Error handling

Whenever an error occurs within the PE, it will raise an error just before transitioning to PlaybackState `IDLE`.

Even though it is implicitly given by the text above, it is important to note that errors related to media product X
shall only be raised within the timeframe where X is the active media product (or should have been). This is a design
decision aimed at simplifying error handling in the app, since this means the app can always assume that playback has
stopped due to an error when receiving an error from PE.

## Media product source for playback

This section outlines what source to use (offline storage/streaming) when a playback is requested. There are multiple
scenarios to take into account here. App being in offline mode or not, media product being offlined or not and it is
important that the Player behaves in a predictable way when choosing the source for playing a media product.

Here we’ll go through the different scenarios and how we handle them.

* App in online mode, requested media product is not offlined
    * The Player shall make a playbackinfo request and stream the requested media product online
* App in online mode, requested media product is offlined
    * The Player shall play the offlined version of the requested media product and the Player shall not even make a
      playbackinfo request in this scenario. Theoretically this can lead to the user experiencing a different
      AssetPresentation, AudioMode and Quality compared to what is selected for streaming, but practically this will
      currently only affect Quality (you can get a different quality during playback than what you've selected for
      streaming, something that is currently considered acceptable). If the requested media product is not valid for
      playback, e.g. current wall clock time < playbackinfo.offlineValidUntil, a corresponding error shall be raised.
* App in offline mode, requested media product is not offlined
    * This is considered an illegal playback request and the Player shall raise a corresponding error.
* App in offline mode, requested media product is offlined
    * The Player shall play the offlined version of the requested media product, given that it is valid for playback (
      e.g. current wall clock time < playbackinfo.offlineValidUntil). If the media product is not valid for playback,
      this is considered an illegal playback request and PE shall raise a corresponding error.

## Gapless audio playback

Gapless audio playback means playing two consecutive media products without any audible gap between them. There are
multiple albums where consecutive tracks are intended to be presented to the user without any audible gap between them.
In order for us to give the end user the experience the artist intended, it is important that we support this feature.

The client must handle encoder delay metadata signaled in MP4 files correctly and possibly adjust for "empty" frames
added by the encoder.

* A player must support and respect EditBox metadata kept in the fMP4 (ISO-BMFF) container.
* A player must have a buffering model that allows for seamless playback between two consecutive audio tracks.

## Loudness normalization

Loudness normalization is the act of normalizing the loudness level of media products during playback so that the user
experiences a similar loudness level. This is an important feature, allowing the end user to perceive a similar loudness
level, regardless of the native loudness of the content.

The Player shall support loudness normalization for all types of media products containing an audio stream. The app
can choose which loudness normalization mode to use via the PE configuration API.

There are four fields in the playbackinfo response that are relevant for loudness normalization:

* `albumReplayGain` - Replay gain for entire album that media product belongs to
* `albumPeakAmplitude` - Peak amplitude for entire album that media product belongs to
* `trackReplayGain` - Replay gain for media product
* `trackPeakAmplitude` - Peak amplitude for media product

We aim at making these four fields available in playbackinfo responses for all media products, but there is currently no
guarantee for that. If a field needed for loudness normalization is missing, the Player shall just fall back to
playback without loudness normalization for that media product.

The Player shall use so-called "reduced gain" when applying loudness normalization. An overall scale factor (Sf) for
loudness normalization taking into account replay gain, pre-amp setting and clipping prevention through gain reduction
is given below.

$$S_f = min \left( 10 ^ \frac {RG+G_{preamp}} {20}, \frac {1} {peakamplitude} \right)$$

Where:

* $`S_f`$ = Overall loudness normalization scale factor. The factor to scale the volume with, in order to achieve
  normalized loudness.
* $`RG`$ = Replay gain. (albumReplayGain for album style normalization, trackReplayGain for track style normalization)
* $`peakamplitude`$ = Peak amplitude. (albumPeakAmplitude for album style normalization, trackPeakAmplitude for track
  style normalization)
* $`G_{preamp}`$ = Pre amplification. A value that can be used to target a different LUFS than the default -14dB LUFS
  that ReplayGain uses. This is configurable in the API.

This equation above can give both positive and negative values. A positive value indicates that the loudness of the
media product shall be increased during playback, so-called positive gain. A negative value indicates that the loudness
of the media product shall be decreased during playback, so-called negative gain.

Negative gain is usually unproblematic to handle since that just means lowering the volume. Positive gain, however, is a
bit more tricky. In order to support that we need to make sure to have some headroom so that we actually can increase
the volume. In order to achieve this we will use an internal volume control in the player, which sets the player volume
relative to the system volume. Some players, like exo player and AVPlayer, have built-in support for this. For other
players we probably have to wrap the application/system volume in an additional home-grown layer to build this.

Analysis of existing replay gain and peak values in our catalog indicates that a base player volume ($`PV_b`$) of 0.8
gives us a good trade-off between keeping it as high as possible, but still being able to increase volume for a big
enough part of our catalog. When loudness normalization is OFF, $`PV_b`$ shall be set to 1.

Here is a summary of how we generated and interpreted the data used to decide $`PV_b`$.

```python
# Code executed in Databricks
# Calculate reduced gain scale factor (rg_sf). Cap values to range [0, 3]
from pyspark.sql.functions import least, greatest

rg_sf_df = amw_track_df.select(least(pow(10, amw_track_df.replaygain_gain/20), 1/amw_track_df.replaygain_peak).alias("rg_sf"))

rg_sf_capped_df = rg_sf_df.select(greatest(lit(0), least(lit(3), rg_sf_df.rg_sf)).alias("rg_sf_capped"))

rg_sf_capped_df.display()
```

The resulting table was then presented as two histograms with 30 and 1000 buckets respectively where:

* X-axis: Calculated scale factor.
* Y-axis: percentage of tracks that have a scale factor in the corresponding bucket.

![Replay Gain](https://github.com/tidal-music/tidal-sdk/blob/main/media/player-reduced-gain-scale-factor.png)

These visualizations show that the 90 percentile of our catalog has a scale factor of 1.2. The 95 percentile has a scale
factor of 1.6. This means that by setting PVb to 0.8, we would be able to correctly normalize > 90% of our catalog,
which we consider a good trade off between lowering the base volume and giving the user a good loudness normalization
experience.

So, actual player volume ($`PV_a`$), when applying loudness normalization shall be calculated as:
$`PV_a`$ = $`PV_b`$ * $`S_f`$

Where:

* $`PV_a`$ = Actual player volume
* $`PV_b`$ = Base player volume: 0.8
* $`S_f`$ = Overall loudness normalization scale factor

# OE Functionality and Requirements

## Re-validation of offlined media products

When a media product has been offlined, we must periodically validate that the media product is still valid for use.
There are multiple things that can happen that would invalidate an offlined media product such as user billing cycle
ends, label requirements, DRM license expires, product takedown, product redelivery etc.

The basic idea here is that the Player will re-validate an offlined media product by making an identical playbackinfo
request as was made when first offlining the media product. By analyzing the response, the Player can figure out if
the media product is still valid, needs to be offlined again or is no longer valid. Note: the Player implementations
shall account for the fact that the system clock might not be correct and can hence not deduct current unix time only
based on that.

Here is a more detailed description of the logic the Player shall implement.

* The Player shall periodically check if offlined media products need re-validating.
    * Re-validations shall respect the Offline over cellular setting, i.e. if Offline over cellular is enabled,
      re-validation can be done no matter internet connection, if not, this logic shall only run over WiFi.
    * Re-validation shall occur at every app start and once every 6h during app execution. Offline over cellular must be
      respected.
    * Re-validations shall be made like transactions, i.e. either a complete successful re-validation occurs, or we
      change nothing.
* For all media products for which current unix time > PBI::offlineRevalidateAt, the Player shall fetch a new
  playbackinfo for the original media product and:
    * If OK response and PBI::manifestHash in the playbackinfo response is identical to the PBI::manifestHash for the
      currently offlined media product
        * If a PBI::licenseSecurityToken exists, a new DRM license must be fetched
        * Manifest and content need not be offlined again.
    * If OK response, but PBI::manifestHash in the playbackinfo response differs from the PBI::manifestHash for the
      currently offlined media product, the entire media product must be re-offlined.
        * The "old" offline must be removed
        * The media product must be re-offlined
        * Offline messages as defined in OE api shall be sent
    * If a non-OK response is received, or any other failures occur, the re-validation is skipped and tried again at the
      next re-validation round.
        * We never remove any offlined media products unless the user explicitly tells us to
        * If no re-validation succeeds until PBI::offlineValidUntil, the media product shall be marked as
          OFFLINED_BUT_NOT_VALID and not be possible to play from the OE.
        * Re-validation shall still be done on this media product, even if current unix time > PBI::offlineValidUntil
