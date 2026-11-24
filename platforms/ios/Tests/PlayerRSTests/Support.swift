import AVFoundation
import Foundation
import XCTest
@testable import PlayerRS

final class Manifests: HttpTransport, @unchecked Sendable {
	private let lock = NSLock()
	private var media: [String: String] = [:]

	func add(_ id: String, _ uri: String) {
		lock.withLock { media[id] = uri }
	}

	func execute(request: HttpRequest) async throws -> HttpResponse {
		let id = URL(string: request.url)!.lastPathComponent
		guard let uri = lock.withLock({ media[id] }) else {
			return HttpResponse(statusCode: 404, body: Data())
		}
		let body = #"{"data":{"id":"\#(id)","attributes":{"uri":"\#(uri)","formats":["AACLC"]}}}"#
		return HttpResponse(statusCode: 200, body: Data(body.utf8))
	}
}

final class Credentials: Authenticator, @unchecked Sendable {
	func accessToken() async throws -> String { "token" }
	func refreshedAccessToken() async throws -> String { "token" }
}

final class Telemetry: TidalEventSender, @unchecked Sendable {
	func sendEvent(event: ReportingEvent) {}
}

final class Events: PlayerEvents, @unchecked Sendable {
	private let lock = NSLock()
	private var events: [PlayerEvent] = []

	func onEvent(event: PlayerEvent) {
		lock.withLock { events.append(event) }
	}

	var all: [PlayerEvent] {
		lock.withLock { events }
	}

	@discardableResult
	func next(
		timeout: TimeInterval = 10,
		file: StaticString = #filePath,
		line: UInt = #line,
		where predicate: (PlayerEvent) -> Bool
	) async throws -> PlayerEvent {
		let deadline = Date().addingTimeInterval(timeout)
		while Date() < deadline {
			let found: PlayerEvent? = lock.withLock {
				guard let index = events.firstIndex(where: predicate) else {
					return nil
				}
				let event = events[index]
				events.removeFirst(index + 1)
				return event
			}
			if let found {
				return found
			}
			try await Task.sleep(for: .milliseconds(20))
		}
		XCTFail("Timed out waiting for an event; received \(all)", file: file, line: line)
		throw CancellationError()
	}

	func state(_ product: MediaProduct?, _ state: PlayerState, timeout: TimeInterval = 10) async throws {
		try await next(timeout: timeout) {
			switch $0 {
			case let .currentItemChanged(item, current), let .stateChanged(item, current):
				return item == product && current == state
			default:
				return false
			}
		}
	}

	@discardableResult
	func current(_ product: MediaProduct?, timeout: TimeInterval = 10) async throws -> PlayerState {
		let event = try await next(timeout: timeout) {
			if case let .currentItemChanged(item, _) = $0 {
				return item == product
			}
			return false
		}
		guard case let .currentItemChanged(_, state) = event else {
			throw CancellationError()
		}
		return state
	}

	func position(_ product: MediaProduct, _ matches: @escaping (TimeInterval) -> Bool) async throws {
		try await next {
			if case let .positionChanged(item, position, _) = $0 {
				return item == product && matches(position)
			}
			return false
		}
	}
}

final class RecordingFactory: MediaPlayerFactory, @unchecked Sendable {
	private let factory: AVQueueMediaPlayerFactory
	private let gate: Gate?
	private let lock = NSLock()
	private var created: [AVQueueMediaPlayer] = []
	private var recorded: [RecordedMediaEvents] = []

	init(_ factory: AVQueueMediaPlayerFactory, gate: Gate?) {
		self.factory = factory
		self.gate = gate
	}

	var players: [AVQueueMediaPlayer] {
		lock.withLock { created }
	}

	var mediaEvents: [RecordedMediaEvents] {
		lock.withLock { recorded }
	}

	func create(events: MediaPlayerEvents) -> PlayerRS.MediaPlayer {
		let events = RecordedMediaEvents(events)
		let player = factory.create(events: events) as! AVQueueMediaPlayer
		lock.withLock {
			created.append(player)
			recorded.append(events)
		}
		return gate.map { GatedMediaPlayer(player, gate: $0) } ?? player
	}
}

final class RecordedMediaEvents: MediaPlayerEvents, @unchecked Sendable {
	private let events: MediaPlayerEvents
	private let lock = NSLock()
	private var recorded: [MediaPlayerEvent] = []

	init(_ events: MediaPlayerEvents) {
		self.events = events
		super.init(noHandle: .init())
	}

	required init(unsafeFromHandle handle: UInt64) {
		fatalError("Recorded events are created in Swift")
	}

	var all: [MediaPlayerEvent] {
		lock.withLock { recorded }
	}

	override func onEvent(event: MediaEvent) {
		lock.withLock { recorded.append(event.event) }
		events.onEvent(event: event)
	}
}

final class Gate: @unchecked Sendable {
	private let lock = NSLock()
	private var used = false
	private var held: CheckedContinuation<Void, Never>?

	var isHolding: Bool {
		lock.withLock { held != nil }
	}

	func pass() async {
		await withCheckedContinuation { continuation in
			let hold = lock.withLock {
				defer { used = true }
				if !used {
					held = continuation
				}
				return !used
			}
			if !hold {
				continuation.resume()
			}
		}
	}

	func open() {
		lock.withLock {
			defer { held = nil }
			return held
		}?.resume()
	}
}

final class GatedMediaPlayer: PlayerRS.MediaPlayer, @unchecked Sendable {
	private let player: AVQueueMediaPlayer
	private let gate: Gate

	init(_ player: AVQueueMediaPlayer, gate: Gate) {
		self.player = player
		self.gate = gate
	}

	func setCurrentAndPlay(item: MediaItem) async throws {
		await gate.pass()
		try await player.setCurrentAndPlay(item: item)
	}

	func setCurrent(item: MediaItem) async throws { try await player.setCurrent(item: item) }
	func setNext(item: MediaItem?) async throws { try await player.setNext(item: item) }
	func pause() async { await player.pause() }
	func resume() async { await player.resume() }
	func seek(position: TimeInterval) async throws { try await player.seek(position: position) }
	func setVolume(gain: Float) async { await player.setVolume(gain: gain) }
	func reset() async { await player.reset() }
	func dispose() { player.dispose() }
}

struct Host {
	let player: Player
	let events: Events
	let manifests: Manifests
	let factory: RecordingFactory

	init(crossfade: TimeInterval? = nil, gate: Gate? = nil) {
		events = Events()
		manifests = Manifests()
		let drm = FairPlayDrmProvider()
		factory = RecordingFactory(AVQueueMediaPlayerFactory(drmProvider: drm), gate: gate)
		player = Player(
			config: PlayerConfig(
				baseUrl: "https://api.test",
				manifestConfiguration: ManifestConfiguration(
					trackConfiguration: TrackManifestConfiguration(
						manifestType: .hls, audioFormats: [.aacLc], adaptive: false, uriScheme: .https,
						usage: .playback
					),
					videoConfiguration: VideoManifestConfiguration(uriScheme: .https, usage: .playback)
				),
				crossfade: crossfade.map { CrossfadeConfig(overlap: $0) }
			),
			transport: manifests,
			authenticator: Credentials(),
			mediaFactory: factory,
			drmProvider: drm,
			eventSender: Telemetry(),
			events: events
		)
	}

	func seed(_ id: String, _ url: URL) -> PlaybackSeed {
		seed(id, uri: url.absoluteString)
	}

	func seed(_ id: String, uri: String) -> PlaybackSeed {
		manifests.add(id, uri)
		return player.prepare(product: .track(id: id))
	}
}

enum Tones {
	private static let directory = FileManager.default.temporaryDirectory
		.appendingPathComponent("player-rs-tests-\(ProcessInfo.processInfo.processIdentifier)", isDirectory: true)

	static let missing = directory.appendingPathComponent("missing.m4a")

	static func tone(seconds: Double) throws -> URL {
		let url = directory.appendingPathComponent("tone-\(seconds).m4a")
		if FileManager.default.fileExists(atPath: url.path) {
			return url
		}
		try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
		let format = AVAudioFormat(standardFormatWithSampleRate: 44100, channels: 1)!
		let file = try AVAudioFile(
			forWriting: url,
			settings: [
				AVFormatIDKey: kAudioFormatMPEG4AAC,
				AVSampleRateKey: 44100,
				AVNumberOfChannelsKey: 1,
			]
		)
		let frames = AVAudioFrameCount(seconds * format.sampleRate)
		let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: frames)!
		buffer.frameLength = frames
		let samples = buffer.floatChannelData![0]
		for frame in 0 ..< Int(frames) {
			samples[frame] = 0.01 * sin(2 * .pi * 440 * Float(frame) / Float(format.sampleRate))
		}
		try file.write(from: buffer)
		return url
	}
}
