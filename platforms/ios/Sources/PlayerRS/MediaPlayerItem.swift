import AVFoundation
import Foundation

@MainActor
protocol MediaPlayerItemObserver: AnyObject {
	func onStatus(_ item: MediaPlayerItem, _ status: AVPlayerItem.Status)
	func onDuration(_ item: MediaPlayerItem, _ duration: CMTime)
	func onFailedToPlayToEnd(_ item: MediaPlayerItem, _ error: Error?)
}

@MainActor
final class MediaPlayerItem {
	let item: MediaItem
	let playerItem: AVPlayerItem
	var started = false
	private weak var observer: (any MediaPlayerItemObserver)?
	private var observations: [NSKeyValueObservation] = []
	private nonisolated(unsafe) var notification: NSObjectProtocol?

	init(_ item: MediaItem, registry: ContentKeyRegistry, observer: MediaPlayerItemObserver) throws {
		let input = item.mediaInput()
		let asset = try AVURLAsset.create(from: input)
		if let drm = input.drm {
			try registry.attach(asset, drm: drm)
		}
        
		self.item = item
        self.observer = observer
        
		playerItem = AVPlayerItem(asset: asset)
		playerItem.variantPreferences = .scalabilityToLosslessAudio
        
		observations = [
			playerItem.observe(\.status) { [weak self] _, _ in
				guard let self else {
					return
				}
				onMain { self.onPlayerItemStatusChanged() }
			},
			playerItem.observe(\.duration) { [weak self] _, _ in
				guard let self else {
					return
				}
				onMain { self.onPlayerItemDurationChanged() }
			},
		]
		notification = NotificationCenter.default.addObserver(
			forName: AVPlayerItem.failedToPlayToEndTimeNotification,
			object: playerItem,
			queue: .main
		) { [weak self] notification in
			let error = notification.userInfo?[AVPlayerItemFailedToPlayToEndTimeErrorKey] as? Error
			MainActor.assumeIsolated { self?.onPlayerItemFailedToPlayToEnd(error) }
		}
	}

	deinit {
		notification.map(NotificationCenter.default.removeObserver)
	}

	func position() -> PlaybackPosition {
		let position = max(0, playerItem.currentTime().seconds.finiteOrZero)
		return PlaybackPosition(position: position, remaining: duration.map { max(0, $0 - position) } ?? 0)
	}

	var duration: TimeInterval? {
		let duration = playerItem.duration.seconds
		return duration.isFinite ? duration : nil
	}

	var hasVideo: Bool {
		playerItem.tracks.contains { $0.assetTrack?.mediaType == .video }
	}
}

private extension MediaPlayerItem {
	func onPlayerItemStatusChanged() {
		observer?.onStatus(self, playerItem.status)
	}

	func onPlayerItemDurationChanged() {
		observer?.onDuration(self, playerItem.duration)
	}

	func onPlayerItemFailedToPlayToEnd(_ error: Error?) {
		observer?.onFailedToPlayToEnd(self, error)
	}
}

extension AVURLAsset {
	static func create(from mediaInput: MediaInput) throws -> AVURLAsset {
		guard let url = URL(string: mediaInput.url) else {
			throw MediaError.Content(code: "URL", description: "Invalid media URL: \(mediaInput.url)")
		}
        
		return AVURLAsset(url: url)
	}
}

private extension Double {
	var finiteOrZero: Double {
		isFinite ? self : 0
	}
}
