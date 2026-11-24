import Foundation

public final class URLSessionHttpTransport: HttpTransport, @unchecked Sendable {
	private let urlSession: URLSession

	public init(urlSession: URLSession = .shared) {
		self.urlSession = urlSession
	}

	public func execute(request: HttpRequest) async throws -> HttpResponse {
		guard let url = URL(string: request.url) else {
			throw TransportError.OtherError(description: "Invalid URL: \(request.url)")
		}
		var urlRequest = URLRequest(url: url, timeoutInterval: 30)
		urlRequest.httpMethod = request.method.name
		for (name, value) in request.headers {
			urlRequest.setValue(value, forHTTPHeaderField: name)
		}
		urlRequest.httpBody = request.body

		let data: Data
		let response: URLResponse
		do {
			(data, response) = try await urlSession.data(for: urlRequest)
		} catch {
			throw transportError(error)
		}
		guard let response = response as? HTTPURLResponse else {
			throw TransportError.OtherError(description: "Non-HTTP response from \(request.url)")
		}
		return HttpResponse(statusCode: UInt16(clamping: response.statusCode), body: data)
	}
}

private func transportError(_ error: Error) -> TransportError {
	let description = String(describing: error)
	switch (error as? URLError)?.code {
	case .timedOut:
		return .TimeoutError(description: description)
	case .notConnectedToInternet, .networkConnectionLost, .cannotConnectToHost, .cannotFindHost, .dnsLookupFailed:
		return .NetworkError(description: description)
	default:
		return .OtherError(description: description)
	}
}

private extension HttpMethod {
	var name: String {
		switch self {
		case .get: "GET"
		case .post: "POST"
		case .patch: "PATCH"
		case .delete: "DELETE"
		}
	}
}
