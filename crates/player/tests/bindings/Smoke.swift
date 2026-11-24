import Foundation

final class HTTP: HttpTransport, @unchecked Sendable {
  let malformed: Bool
  let lock = NSLock()
  var licenses = 0
  init(malformed: Bool = false) { self.malformed = malformed }
  func execute(request: HttpRequest) async throws -> HttpResponse {
    precondition(request.headers["Authorization"] == "Bearer token")
    // Rejecting the media player's license fetch once makes the core refresh the token,
    // which needs the core's executor rather than the caller's.
    if request.url == "https://license.example" {
      let attempt = lock.withLock { licenses += 1; return licenses }
      if attempt == 2 { return HttpResponse(statusCode: 401, body: Data()) }
    }
    let body =
      malformed
      ? "invalid json"
      : #"{"data":{"id":"42","attributes":{"uri":"https://media.example/track","formats":["FLAC"],"drmData":{"drmSystem":"FAIRPLAY","licenseUrl":"https://license.example","certificateUrl":"https://certificate.example","initData":["init"]}}}}"#
    return HttpResponse(statusCode: 200, body: Data(body.utf8))
  }
}
final class Auth: Authenticator, @unchecked Sendable {
  func accessToken() async throws -> String { "token" }
  func refreshedAccessToken() async throws -> String { "token" }
}
final class Session: LicenseSession, @unchecked Sendable {
  func certificateRequest() -> CertificateRequest { .get }
  func createChallenge(certificate: Data, initData: Data?) async throws -> Data {
    precondition(initData == Data("init".utf8))
    return Data("challenge".utf8)
  }
  func installLicense(license: Data) async throws -> Data { Data("handle".utf8) }
}
final class DRM: DrmProvider, @unchecked Sendable {
  func preloadsLicenses() -> Bool { true }
  func createSession() -> LicenseSession { Session() }
}
final class Telemetry: TidalEventSender, @unchecked Sendable {
  func sendEvent(event: ReportingEvent) {}
}
final class Events: PlayerEvents, @unchecked Sendable {
  private let condition = NSCondition()
  private var events: [PlayerEvent] = []
  func onEvent(event: PlayerEvent) {
    condition.lock()
    events.append(event)
    condition.broadcast()
    condition.unlock()
  }
  func waitFor(_ predicate: (PlayerEvent) -> Bool) -> PlayerEvent {
    condition.lock()
    defer { condition.unlock() }
    let deadline = Date().addingTimeInterval(5)
    while true {
      if let event = events.first(where: predicate) { return event }
      precondition(condition.wait(until: deadline), "event timed out")
    }
  }
}
final class Media: MediaPlayer, @unchecked Sendable {
  let events: MediaPlayerEvents
  let released: DispatchSemaphore
  let lock = NSLock()
  var disposeCalls = 0
  var disposed = false
  init(_ events: MediaPlayerEvents, _ released: DispatchSemaphore) {
    self.events = events
    self.released = released
  }
  func dispose() {
    lock.lock()
    defer { lock.unlock() }
    disposeCalls += 1
    if disposed { return }
    disposed = true
    released.signal()
  }
  private func emit(event: MediaPlayerEvent) {
    events.onEvent(event: MediaEvent(event: event, emittedAt: Date()))
  }
  func setCurrentAndPlay(item: MediaItem) async throws {
    guard case .prepared(let handle) = item.mediaInput().drm, handle == Data("handle".utf8) else {
      preconditionFailure("expected the prepared license")
    }
    // An engine may still ask for a license; the item fetches it.
    _ = try await item.fetchCertificate(request: .get)
    _ = try await item.fetchLicense(challenge: Data("challenge".utf8))
    emit(event: .prepared(item))
    emit(event: .current(item: item))
    emit(event: .duration(item, 120.5))
    emit(event: .seeking(item, 0))
    emit(event: .seeked(item, PlaybackPosition(position: 1.25, remaining: 119.25)))
    emit(event: .started(item, 1.25))
    emit(event: .position(item, PlaybackPosition(position: 1.25, remaining: 119.25)))
  }
  func setCurrent(item: MediaItem) async throws {
    emit(event: .prepared(item))
    emit(event: .current(item: item))
  }
  func setNext(item: MediaItem?) async throws {}
  func pause() async {}
  func resume() async {}
  func seek(position: TimeInterval) async throws {}
  func setVolume(gain: Float) async {}
  func reset() async {}
}
final class Factory: MediaPlayerFactory, @unchecked Sendable {
  let released = DispatchSemaphore(value: 0)
  var retained: [Media] = []
  func create(events: MediaPlayerEvents) -> MediaPlayer {
    let media = Media(events, released)
    retained.append(media)
    return media
  }
}
@main struct Smoke {
  static func main() async throws {
    let config = PlayerConfig(
      baseUrl: "https://api.example",
      manifestConfiguration: ManifestConfiguration(
        trackConfiguration: TrackManifestConfiguration(
          manifestType: .hls, audioFormats: [.flac], adaptive: false, uriScheme: .https,
          usage: .playback),
        videoConfiguration: VideoManifestConfiguration(uriScheme: .https, usage: .playback)),
      crossfade: nil)
    let events = Events()
    let factory = Factory()
    var player: Player? = Player(
      config: config, transport: HTTP(), authenticator: Auth(),
      mediaFactory: factory, drmProvider: DRM(), eventSender: Telemetry(), events: events)
    let seed = player!.prepare(product: .track(id: "42"))
    try await player!.play(seed: seed)
    _ = events.waitFor { event in
      // Playing is reported with the selection or, if the media starts later, as a state change.
      switch event {
      case .currentItemChanged(let item, .playing), .stateChanged(let item, .playing):
        return item == .track(id: "42")
      default: return false
      }
    }
    _ = events.waitFor { event in
      if case .positionChanged(let item, let position, let duration) = event {
        return item == .track(id: "42") && position == 1.25 && duration == 120.5
      }
      return false
    }
    await player!.reset()
    _ = events.waitFor { event in
      if case .currentItemChanged(item: nil, state: .idle) = event { return true }
      return false
    }
    player = nil
    precondition(factory.released.wait(timeout: .now() + 5) == .success)
    precondition(factory.retained.allSatisfy { $0.disposeCalls == 1 })

    let failures = Events()
    let failingFactory = Factory()
    var failing: Player? = Player(
      config: config, transport: HTTP(malformed: true), authenticator: Auth(),
      mediaFactory: failingFactory, drmProvider: DRM(), eventSender: Telemetry(), events: failures)
    let badSeed = failing!.prepare(product: .track(id: "bad"))
    do {
      try await failing!.play(seed: badSeed)
      preconditionFailure("Expected playback to fail")
    } catch let error as PlayerError {
      guard case .Other(let canRetry, let code, _) = error else {
        preconditionFailure("Expected a typed manifest error")
      }
      precondition(!canRetry && code == "mdec")
    }
    _ = failures.waitFor { event in
      if case .stateChanged(let item, .failed(let error)) = event {
        if case .Other(let canRetry, let code, _) = error {
          return item == .track(id: "bad") && !canRetry && code == "mdec"
        }
      }
      return false
    }
    failing = nil
    precondition(failingFactory.released.wait(timeout: .now() + 5) == .success)
    print("PASS Swift: typed state/position events, failure callbacks, DRM, items and release")
  }
}
