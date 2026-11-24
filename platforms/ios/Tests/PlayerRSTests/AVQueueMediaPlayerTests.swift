import AVFoundation
import XCTest
@testable import PlayerRS

final class AVQueueMediaPlayerTests: XCTestCase {
	private let a = MediaProduct.track(id: "a")
	private let b = MediaProduct.track(id: "b")

	func testPlaysPausesSeeksAndResets() async throws {
		let host = Host()
		try await host.player.play(seed: host.seed("a", Tones.tone(seconds: 5)))
		try await host.events.state(a, .playing)
		try await host.events.position(a) { $0 > 0.2 }

		await host.player.pause()
		try await host.events.state(a, .paused)
		try await host.player.seek(position: 3)
		try await host.events.position(a) { $0 > 1 }

		await host.player.resume()
		try await host.events.state(a, .playing)
		try await host.events.position(a) { $0 > 3.1 }

		await host.player.reset()
		try await host.events.state(nil, .idle)
		XCTAssertTrue(host.factory.players[0].player.items().isEmpty)
	}

	func testDoesNotReportPositionWhileSeeking() async throws {
		let host = Host()
		try await host.player.play(seed: host.seed("a", Tones.tone(seconds: 5)))
		try await host.events.position(a) { $0 > 0.2 }

		try await host.player.seek(position: 3)
		try await host.events.position(a) { $0 > 3.1 }

		let events = host.factory.mediaEvents[0].all
		let seeking = try XCTUnwrap(events.firstIndex { if case .seeking = $0 { true } else { false } })
		let seeked = try XCTUnwrap(events.firstIndex { if case .seeked = $0 { true } else { false } })
		XCTAssertFalse(events[seeking..<seeked].contains { if case .position = $0 { true } else { false } })
	}

	func testAdvancesToTheQueuedNextItemAndCompletes() async throws {
		let host = Host()
		try await host.player.play(seed: host.seed("a", Tones.tone(seconds: 1)))
		try await host.player.setNext(seed: host.seed("b", Tones.tone(seconds: 1)))

		try await host.events.state(b, .playing)
		try await host.events.position(b) { $0 > 0.2 }
		try await host.events.state(nil, .completed)
		XCTAssertEqual(host.factory.players.count, 1)
	}

	func testNextItemSetAfterCompletionPlays() async throws {
		let host = Host()
		try await host.player.play(seed: host.seed("a", Tones.tone(seconds: 1)))
		try await host.events.state(nil, .completed)

		try await host.player.setNext(seed: host.seed("b", Tones.tone(seconds: 1)))
		try await host.events.state(b, .playing)
	}

	func testClearedNextItemIsNotPlayed() async throws {
		let host = Host()
		try await host.player.play(seed: host.seed("a", Tones.tone(seconds: 1)))
		try await host.player.setNext(seed: host.seed("b", Tones.tone(seconds: 1)))
		try await host.player.setNext(seed: nil)

		try await host.events.state(nil, .completed)
		XCTAssertFalse(host.events.all.contains { if case .currentItemChanged(b, _) = $0 { true } else { false } })
	}

	func testRejectedNextItemKeepsTheStagedOne() async throws {
		let host = Host()
		try await host.player.play(seed: host.seed("a", Tones.tone(seconds: 1)))
		try await host.player.setNext(seed: host.seed("b", Tones.tone(seconds: 1)))
		do {
			try await host.player.setNext(seed: host.seed("c", uri: ""))
			XCTFail("Expected the next item to be rejected")
		} catch is PlayerError {}

		try await host.events.current(b)
		try await host.events.state(nil, .completed)
	}

	func testFailedNextItemBecomesCurrentWhenPlaybackReachesIt() async throws {
		let host = Host()
		try await host.player.play(seed: host.seed("a", Tones.tone(seconds: 1)))
		try await host.player.setNext(seed: host.seed("b", Tones.missing))

		guard case .failed = try await host.events.current(b) else {
			return XCTFail("Expected the failed next item to become current as failed")
		}
	}

	func testItemThatCannotLoadIsReportedAsFailed() async throws {
		let host = Host()
		try await host.player.play(seed: host.seed("a", Tones.missing))
		try await host.events.next {
			if case let .stateChanged(a, .failed(error)) = $0 {
				return a == self.a && error.isPlaybackError
			}
			return false
		}
	}

	func testPlayingAnotherItemReplacesTheCurrentOne() async throws {
		let host = Host()
		try await host.player.play(seed: host.seed("a", Tones.tone(seconds: 5)))
		try await host.events.position(a) { $0 > 0.2 }

		try await host.player.play(seed: host.seed("b", Tones.tone(seconds: 5)))
		try await host.events.state(b, .playing)
		try await host.events.position(b) { $0 > 0.2 }
		XCTAssertEqual(host.factory.players[0].player.items().count, 1)
	}

	func testCommandCancelledBeforeItRunsDoesNothing() async throws {
		let gate = Gate()
		let host = Host(gate: gate)
		let seedA = try host.seed("a", Tones.tone(seconds: 5))
		let first = Task { try await host.player.play(seed: seedA) }
		while !gate.isHolding {
			try await Task.sleep(for: .milliseconds(10))
		}

		try await host.player.play(seed: host.seed("b", Tones.tone(seconds: 5)))
		try await host.events.state(b, .playing)
		gate.open()
		_ = await first.result
		try await host.events.position(b) { $0 > 0.5 }

		XCTAssertEqual(host.factory.players[0].player.items().count, 1)
		XCTAssertFalse(host.events.all.contains { if case .currentItemChanged(a, _) = $0 { true } else { false } })
	}

	func testCancelledResetDoesNothing() async throws {
		let host = Host()
		try await host.player.play(seed: host.seed("a", Tones.tone(seconds: 5)))
		try await host.events.state(a, .playing)
		let media = host.factory.players[0]
		let gate = Gate()
		let reset = Task {
			await gate.pass()
			await media.reset()
		}
		while !gate.isHolding {
			try await Task.sleep(for: .milliseconds(10))
		}
		reset.cancel()
		gate.open()
		await reset.value

		try await host.events.position(a) { $0 > 0.5 }
		XCTAssertEqual(media.player.items().count, 1)
	}

	func testCrossfadesToTheNextItem() async throws {
		let host = Host(crossfade: 0.5)
		try await host.player.play(seed: host.seed("a", Tones.tone(seconds: 1.5)))
		try await host.player.setNext(seed: host.seed("b", Tones.tone(seconds: 1.5)))

		let state = try await host.events.current(b)
		XCTAssertEqual(state, .playing)
		try await host.events.state(nil, .completed)
		XCTAssertEqual(host.factory.players.count, 2)
	}

	func testReleasingThePlayerDisposesItsMediaPlayers() async throws {
		var host: Host? = Host()
		try await host!.player.play(seed: host!.seed("a", Tones.tone(seconds: 5)))
		try await host!.events.state(a, .playing)
		let players = host!.factory.players.map(\.player)

		host = nil
		let deadline = Date().addingTimeInterval(5)
		while players.contains(where: { !$0.items().isEmpty }), Date() < deadline {
			try await Task.sleep(for: .milliseconds(20))
		}
		XCTAssertTrue(players.allSatisfy { $0.items().isEmpty && $0.rate == 0 })
	}
}

private extension PlayerError {
	var isPlaybackError: Bool {
		switch self {
		case let .Network(code, _), let .Other(_, code, _), let .Denied(_, code, _):
			code.hasPrefix("p")
		}
	}
}
