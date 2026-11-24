import AVFoundation
import Foundation

public final class AVQueueMediaPlayerFactory: MediaPlayerFactory, Sendable {
	private let registry: ContentKeyRegistry
	private let onVideo: @Sendable (AVPlayer) -> Void

	public init(drmProvider: FairPlayDrmProvider, onVideo: @escaping @Sendable (AVPlayer) -> Void = { _ in }) {
		registry = drmProvider.registry
		self.onVideo = onVideo
	}

	public func create(events: MediaPlayerEvents) -> MediaPlayer {
		AVQueueMediaPlayer(events: events, registry: registry, onVideo: onVideo)
	}
}
