import AVFoundation
import Foundation

func mediaError(_ error: Error?) -> MediaError {
	guard let error else {
		return .Other(code: "UNKNOWN", description: "Playback failed without an error")
	}
	if let error = error as? MediaError {
		return error
	}
	let description = String(describing: error)
	let chain = sequence(first: error as NSError) { $0.userInfo[NSUnderlyingErrorKey] as? NSError }
	for error in chain {
		let code = String(error.code)
		switch error.domain {
		case NSURLErrorDomain:
			return .Network(code: code, description: description)
		case "CoreMediaErrorDomain" where error.code == -12889:
			return .Network(code: code, description: description)
		case AVFoundationErrorDomain where contentErrors.contains(AVError.Code(rawValue: error.code)):
			return .Content(code: code, description: description)
		default:
			continue
		}
	}
	return .Other(code: String((error as NSError).code), description: description)
}

private let contentErrors: Set<AVError.Code?> = [
	.contentIsUnavailable,
	.contentIsNotAuthorized,
	.decodeFailed,
	.failedToParse,
	.fileFormatNotRecognized,
	.formatUnsupported,
	.noLongerPlayable,
	.undecodableMediaData,
]
