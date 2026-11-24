import AVFoundation
import Foundation

public final class FairPlayDrmProvider: DrmProvider, Sendable {
	let registry = ContentKeyRegistry()

	public init() {}

	public func preloadsLicenses() -> Bool {
		true
	}

	public func createSession() -> LicenseSession {
		#if targetEnvironment(simulator)
			// AVContentKeySession raises an Objective-C exception on simulators, which would abort the app.
			SimulatorLicenseSession()
		#else
			FairPlayLicenseSession(registry: registry)
		#endif
	}
}

#if targetEnvironment(simulator)
	final class SimulatorLicenseSession: LicenseSession {
		func certificateRequest() -> CertificateRequest {
			.get
		}

		func createChallenge(certificate: Data, initData: Data?) async throws -> Data {
			throw DrmError.Failed(description: "FairPlay Streaming is not supported on simulators", code: nil)
		}

		func installLicense(license: Data) async throws -> Data {
			throw DrmError.Failed(description: "FairPlay Streaming is not supported on simulators", code: nil)
		}
	}
#endif

final class FairPlayLicenseSession: NSObject, LicenseSession, @unchecked Sendable {
	private let registry: ContentKeyRegistry
	private let queue = DispatchQueue(label: "com.tidal.player.content-keys")
	private let session = AVContentKeySession(keySystem: .fairPlayStreaming)
	private var certificate = Data()
	private var request: AVContentKeyRequest?
	private var requestError: Error?
	private var licenseChallengeContinuation: CheckedContinuation<Data, Error>?
	private var licenseInstallationContinuation: CheckedContinuation<Void, Error>?

	init(registry: ContentKeyRegistry) {
		self.registry = registry
		super.init()
		session.setDelegate(self, queue: queue)
	}

	func certificateRequest() -> CertificateRequest {
		.get
	}

	func createChallenge(certificate: Data, initData: Data?) async throws -> Data {
		guard let initData, let identifier = String(data: initData, encoding: .utf8) else {
			throw DrmError.Failed(description: "FairPlay content has no key identifier", code: nil)
		}
		do {
			return try await withCheckedThrowingContinuation { continuation in
				queue.async {
					self.certificate = certificate
					self.licenseChallengeContinuation = continuation
					self.session.processContentKeyRequest(withIdentifier: identifier, initializationData: nil)
				}
			}
		} catch {
			throw drmError(error)
		}
	}

	func installLicense(license: Data) async throws -> Data {
		do {
			try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
				queue.async {
					if let requestError = self.requestError {
						return continuation.resume(throwing: requestError)
					}
					self.licenseInstallationContinuation = continuation
					self.request?.processContentKeyResponse(AVContentKeyResponse(fairPlayStreamingKeyResponseData: license))
				}
			}
		} catch {
			throw drmError(error)
		}
        
		return registry.register(self)
	}

	func attach(_ asset: AVURLAsset) {
		session.addContentKeyRecipient(asset)
	}
}

extension FairPlayLicenseSession: AVContentKeySessionDelegate {
	func contentKeySession(_ session: AVContentKeySession, didProvide keyRequest: AVContentKeyRequest) {
		guard let licenseChallengeContinuation else {
			keyRequest.processContentKeyResponseError(
				DrmError.Failed(description: "Only the preloaded FairPlay key is supported", code: nil)
			)
			return
		}
        
		request = keyRequest
		self.licenseChallengeContinuation = nil
		do {
			keyRequest.makeStreamingContentKeyRequestData(
				forApp: certificate,
				contentIdentifier: try keyRequest.keyId(),
				completionHandler: licenseChallengeContinuation.resume
			)
		} catch {
			licenseChallengeContinuation.resume(throwing: error)
		}
	}

	func contentKeySession(_ session: AVContentKeySession, contentKeyRequestDidSucceed keyRequest: AVContentKeyRequest) {
		licenseInstallationContinuation?.resume()
		licenseInstallationContinuation = nil
	}

	func contentKeySession(
		_ session: AVContentKeySession,
		contentKeyRequest keyRequest: AVContentKeyRequest,
		didFailWithError error: Error
	) {
		requestError = error
		licenseChallengeContinuation?.resume(throwing: error)
		licenseChallengeContinuation = nil
		licenseInstallationContinuation?.resume(throwing: error)
		licenseInstallationContinuation = nil
	}
}

final class ContentKeyRegistry: @unchecked Sendable {
	private struct Entry {
		weak var session: FairPlayLicenseSession?
	}

	private let lock = NSLock()
	private var entries: [Data: Entry] = [:]

	func register(_ session: FairPlayLicenseSession) -> Data {
		let handle = Data(UUID().uuidString.utf8)
		lock.withLock {
			entries = entries.filter { $0.value.session != nil }
			entries[handle] = Entry(session: session)
		}
		return handle
	}

	// Keys must be attached before the asset loads.
	func attach(_ asset: AVURLAsset, drm: DrmInput) throws {
		switch drm {
		case let .prepared(handle):
			guard let session = session(for: handle) else {
				throw MediaError.Other(code: "DRM", description: "The prepared licence is no longer available")
			}
			session.attach(asset)
		case .onDemand:
			throw MediaError.Other(code: "DRM", description: "FairPlay licences must be preloaded")
		}
	}

	private func session(for handle: Data) -> FairPlayLicenseSession? {
		lock.withLock { entries[handle]?.session }
	}
}

private extension CheckedContinuation<Data, Error> {
	func resume(_ data: Data?, _ error: Error?) {
		if let data {
			resume(returning: data)
		} else {
			resume(throwing: error ?? DrmError.Failed(description: "No FairPlay challenge", code: nil))
		}
	}
}

private extension AVContentKeyRequest {
	func keyId() throws -> Data {
		guard let identifier = identifier as? String, let host = URL(string: identifier)?.host else {
			throw DrmError.Failed(description: "Invalid FairPlay key identifier: \(String(describing: identifier))", code: nil)
		}
        
		return Data(host.utf8)
	}
}

private func drmError(_ error: Error) -> DrmError {
	if let error = error as? DrmError {
		return error
	}
    
	return .Failed(description: String(describing: error), code: String((error as NSError).code))
}
