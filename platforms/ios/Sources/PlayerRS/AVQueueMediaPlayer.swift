import AVFoundation
import Foundation

private let positionInterval = CMTime(value: 1, timescale: 10)

@MainActor
final class AVQueueMediaPlayer: MediaPlayer {
	nonisolated let player = AVQueuePlayer()
	private nonisolated let events: MediaPlayerEvents
	private nonisolated let registry: ContentKeyRegistry
	private nonisolated let onVideo: @Sendable (AVPlayer) -> Void
	private var current: MediaPlayerItem?
	private var next: NextMediaPlayerItem?
	private var pendingSeeks = 0
	private var observations: [NSKeyValueObservation] = []
	private var timeObserver: Any?

	nonisolated init(events: MediaPlayerEvents, registry: ContentKeyRegistry, onVideo: @escaping @Sendable (AVPlayer) -> Void) {
		self.events = events
		self.registry = registry
		self.onVideo = onVideo
		onMain { [weak self] in
			self?.observePlayer()
		}
	}

	// MARK: MediaPlayer

	func setCurrentAndPlay(item: MediaItem) throws {
		try Task.checkCancellation()
		try replaceItems(with: item)
		player.play()
	}

	func setCurrent(item: MediaItem) throws {
		try Task.checkCancellation()
		try replaceItems(with: item)
	}

	func setNext(item: MediaItem?) throws {
		try Task.checkCancellation()
		guard let item else {
			return removeNext()
		}
		guard let current else {
			return try replaceItems(with: item)
		}
        
		let next = try MediaPlayerItem(item, registry: registry, observer: self)
		removeNext()
		self.next = .queued(next)
		emit(.preparing(item))
		player.insert(next.playerItem, after: current.playerItem)
	}

	func pause() {
		guard !Task.isCancelled else {
			return
		}
		player.pause()
	}

	func resume() {
		guard !Task.isCancelled else {
			return
		}
		player.play()
	}

	func seek(position: TimeInterval) async {
		guard !Task.isCancelled, let current else {
			return
		}
        
		emit(.seeking(current.item, current.position().position))
		pendingSeeks += 1
		let finished = await current.playerItem.seek(to: CMTime(seconds: position, preferredTimescale: 1000))
		pendingSeeks -= 1
		guard finished, current === self.current else {
			return
		}
        
		emit(.seeked(current.item, current.position()))
	}

	func setVolume(gain: Float) {
		guard !Task.isCancelled else {
			return
		}
        
		player.volume = gain
	}

	func reset() {
		guard !Task.isCancelled else {
			return
		}
        
		player.pause()
		releaseItems()
	}

	nonisolated func dispose() {
		onMain { [self] in
			tearDown()
		}
	}
}

private enum NextMediaPlayerItem {
	case queued(MediaPlayerItem)
	case failed(MediaItem)

	var queued: MediaPlayerItem? {
		if case let .queued(item) = self { item } else { nil }
	}
}

private extension AVQueueMediaPlayer {
	// MARK: Placement

	func observePlayer() {
		observations = [
			player.observe(\.timeControlStatus) { [weak self] _, _ in
				guard let self else {
					return
				}
				onMain { self.onPlayerStatusChanged() }
			},
			player.observe(\.currentItem, options: .old) { [weak self] _, change in
				guard let self else {
					return
				}
				let left = change.oldValue ?? nil
				onMain { self.onPlayerItemChanged(from: left) }
			},
		]
		timeObserver = player.addPeriodicTimeObserver(forInterval: positionInterval, queue: .main) { [weak self] _ in
			MainActor.assumeIsolated { self?.onPlayerPositionChanged() }
		}
	}

	func replaceItems(with item: MediaItem) throws {
		let current = try MediaPlayerItem(item, registry: registry, observer: self)
		releaseItems()
		self.current = current
		emit(.preparing(item))
		emit(.current(item: item))
		player.volume = item.volume()
		player.insert(current.playerItem, after: nil)
	}

	func removeNext() {
		next?.queued.map { player.remove($0.playerItem) }
		next = nil
	}

	func releaseItems() {
		current = nil
		next = nil
		player.removeAllItems()
	}

	func tearDown() {
		player.pause()
		releaseItems()
		observations.forEach { $0.invalidate() }
		timeObserver.map(player.removeTimeObserver)
		timeObserver = nil
	}

	// MARK: Player observations

	func onPlayerItemChanged(from left: AVPlayerItem?) {
		guard let current, left === current.playerItem else {
			return
		}
        
		if current.playerItem.status == .failed {
			onItemFailed(current, current.playerItem.error)
			return
		}
        
		emit(.ended(current.item, current.duration ?? current.position().position))
        
		let next = self.next
		self.current = next?.queued
		self.next = nil
        
		switch next {
		case nil:
			emit(.completed)
		case let .failed(item):
			emit(.current(item: item))
		case let .queued(next):
			emit(.current(item: next.item))
			player.volume = next.item.volume()
			if player.timeControlStatus == .playing {
				onItemStarted(next)
			}
		}
	}

	func onPlayerPositionChanged() {
		guard
			pendingSeeks == 0,
			let current,
			player.currentItem === current.playerItem,
			player.timeControlStatus == .playing
		else {
			return
		}
        
		emit(.position(current.item, current.position()))
	}

	func onPlayerStatusChanged() {
		guard let current, player.currentItem === current.playerItem else {
			return
		}
        
		switch player.timeControlStatus {
		case .playing where current.started:
			emit(.resumed(current.item, current.position()))
		case .playing:
			onItemStarted(current)
		case .paused:
			emit(.paused(current.item, current.position()))
		case .waitingToPlayAtSpecifiedRate where player.reasonForWaitingToPlay == .toMinimizeStalls:
			emit(.stalled(current.item, current.position()))
		default:
			break
		}
	}

	// MARK: Item observations

	func onItemStarted(_ item: MediaPlayerItem) {
		item.started = true
		emit(.started(item.item, item.position().position))
		if item.hasVideo {
			onVideo(player)
		}
	}

	func onItemFailed(_ item: MediaPlayerItem, _ error: Error?) {
		emit(.failed(item.item, item.position().position, mediaError(error)))
		if item === current {
			releaseItems()
		} else {
			next = .failed(item.item)
			player.remove(item.playerItem)
		}
	}

	// MARK: Helpers

	func holds(_ item: MediaPlayerItem) -> Bool {
		item === current || item === next?.queued
	}

	func emit(_ event: MediaPlayerEvent) {
		events.onEvent(event: MediaEvent(event: event, emittedAt: Date()))
	}
}

extension AVQueueMediaPlayer: MediaPlayerItemObserver {
	func onStatus(_ item: MediaPlayerItem, _ status: AVPlayerItem.Status) {
		guard holds(item) else {
			return
		}
        
		switch status {
		case .readyToPlay:
			emit(.prepared(item.item))
		case .failed:
			onItemFailed(item, item.playerItem.error)
		default:
			break
		}
	}

	func onDuration(_ item: MediaPlayerItem, _ duration: CMTime) {
		guard holds(item), duration.seconds.isFinite else {
			return
		}
        
		emit(.duration(item.item, duration.seconds))
	}

	func onFailedToPlayToEnd(_ item: MediaPlayerItem, _ error: Error?) {
		guard holds(item) else {
			return
		}
        
		onItemFailed(item, error)
	}
}

nonisolated func onMain(_ body: @escaping @MainActor @Sendable () -> Void) {
	if Thread.isMainThread {
		MainActor.assumeIsolated(body)
	} else {
		DispatchQueue.main.async(execute: body)
	}
}
