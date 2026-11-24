use std::sync::Arc;
use std::time::Duration;

use crate::model::Error as PlayerError;
use crate::platforms::authenticator::{self, Authenticator};
use crate::platforms::transport::{self, HttpMethod, HttpRequest, HttpResponse, HttpTransport};

pub(crate) struct HttpClient {
    transport: Arc<dyn HttpTransport>,
    authenticator: Option<Arc<dyn Authenticator>>,
}

impl HttpClient {
    pub(crate) fn new(
        transport: Arc<dyn HttpTransport>,
        authenticator: Option<Arc<dyn Authenticator>>,
    ) -> Self {
        Self {
            transport,
            authenticator,
        }
    }

    pub(crate) async fn execute(&self, mut request: HttpRequest) -> Result<HttpResponse, Error> {
        let retryable = request.is_retryable();
        let mut response_backoff = Backoff::new(&RESPONSE_BACKOFF);
        let mut network_backoff = Backoff::new(&NETWORK_BACKOFF);
        let mut timeout_backoff = Backoff::new(&TIMEOUT_BACKOFF);
        let mut auth_backoff = Backoff::new(&AUTH_BACKOFF);
        let mut refreshed_token: Option<String> = None;
        loop {
            if let Some(authenticator) = &self.authenticator {
                let token = match refreshed_token.take() {
                    Some(token) => token,
                    None => authenticator.access_token().await?,
                };
                request
                    .headers
                    .insert("Authorization".to_string(), format!("Bearer {token}"));
            }
            match self.transport.execute(request.clone()).await {
                Ok(response) => {
                    if response.status_code == UNAUTHORIZED_STATUS_CODE {
                        if let Some(authenticator) = &self.authenticator {
                            if auth_backoff.wait().await {
                                let Ok(fresh) = authenticator.refreshed_access_token().await else {
                                    return Ok(response);
                                };
                                refreshed_token = Some(fresh);
                                continue;
                            }
                        }
                    }
                    if retryable && response.is_retryable() && response_backoff.wait().await {
                        continue;
                    }
                    return Ok(response);
                }
                Err(error) => {
                    let backoff = match error {
                        transport::Error::NetworkError { .. } => Some(&mut network_backoff),
                        transport::Error::TimeoutError { .. } => Some(&mut timeout_backoff),
                        transport::Error::OtherError { .. } => None,
                    };
                    if let Some(backoff) = backoff {
                        if retryable && backoff.wait().await {
                            continue;
                        }
                    }
                    return Err(error.into());
                }
            }
        }
    }
}

const UNAUTHORIZED_STATUS_CODE: u16 = 401;

trait Retryable {
    fn is_retryable(&self) -> bool;
}

impl Retryable for HttpRequest {
    fn is_retryable(&self) -> bool {
        self.method == HttpMethod::Get
    }
}

impl Retryable for HttpResponse {
    fn is_retryable(&self) -> bool {
        self.status_code == 429 || (500..600).contains(&self.status_code)
    }
}

struct BackoffPolicy {
    base: Duration,
    cap: Duration,
    max_retries: u32,
}

const RESPONSE_BACKOFF: BackoffPolicy = BackoffPolicy {
    base: Duration::from_millis(500),
    cap: Duration::from_secs(16),
    max_retries: 3,
};
const NETWORK_BACKOFF: BackoffPolicy = BackoffPolicy {
    base: Duration::from_secs(1),
    cap: Duration::from_secs(16),
    max_retries: 10,
};
const TIMEOUT_BACKOFF: BackoffPolicy = BackoffPolicy {
    base: Duration::from_secs(8),
    cap: Duration::from_secs(32),
    max_retries: 3,
};
const AUTH_BACKOFF: BackoffPolicy = BackoffPolicy {
    base: Duration::ZERO,
    cap: Duration::ZERO,
    max_retries: 1,
};

struct Backoff {
    policy: &'static BackoffPolicy,
    attempts: u32,
}

impl Backoff {
    fn new(policy: &'static BackoffPolicy) -> Self {
        Self {
            policy,
            attempts: 0,
        }
    }

    async fn wait(&mut self) -> bool {
        if self.attempts >= self.policy.max_retries {
            return false;
        }
        let exponential = self
            .policy
            .base
            .as_millis()
            .saturating_mul(1u128 << self.attempts.min(32));
        let capped = exponential.min(self.policy.cap.as_millis()) as f64;
        self.attempts += 1;
        channel::runtime::sleep(Duration::from_millis((capped * jitter_factor()) as u64)).await;
        true
    }
}

fn jitter_factor() -> f64 {
    0.8 + 0.2 * fastrand::f64()
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Transport(#[from] transport::Error),
    #[error(transparent)]
    Auth(#[from] authenticator::Error),
}

impl From<&Error> for PlayerError {
    fn from(error: &Error) -> Self {
        match error {
            Error::Transport(error) => Self::from(error),
            Error::Auth(error) => Self::from(error),
        }
    }
}

#[cfg(test)]
mod jitter_tests {
    use super::jitter_factor;

    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn jitter_varies_within_the_existing_delay_range() {
        let first = jitter_factor();
        assert!((0.8..=1.0).contains(&first));
        let mut varied = false;
        for _ in 0..128 {
            let factor = jitter_factor();
            assert!((0.8..=1.0).contains(&factor));
            varied |= factor != first;
        }
        assert!(varied, "retry jitter must not be constant");
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    struct FakeTransport {
        requests: Mutex<Vec<HttpRequest>>,
        results: Mutex<Vec<Result<HttpResponse, transport::Error>>>,
    }

    impl FakeTransport {
        fn new(results: Vec<Result<HttpResponse, transport::Error>>) -> Arc<Self> {
            Arc::new(Self {
                requests: Mutex::new(Vec::new()),
                results: Mutex::new(results),
            })
        }

        fn request_count(&self) -> usize {
            self.requests.lock().unwrap().len()
        }
    }
    #[async_trait::async_trait]
    impl HttpTransport for FakeTransport {
        async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, transport::Error> {
            self.requests.lock().unwrap().push(request);
            self.results.lock().unwrap().remove(0)
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum AuthCall {
        Access,
        Refresh,
    }

    struct FakeAuthenticator {
        calls: Mutex<Vec<AuthCall>>,
        tokens: Mutex<Vec<String>>,
        refresh_fails: bool,
    }

    impl FakeAuthenticator {
        fn new(tokens: &[&str]) -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(Vec::new()),
                tokens: Mutex::new(tokens.iter().map(|token| (*token).to_string()).collect()),
                refresh_fails: false,
            })
        }

        fn with_failing_refresh(token: &str) -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(Vec::new()),
                tokens: Mutex::new(vec![token.to_string()]),
                refresh_fails: true,
            })
        }

        fn next_token(&self) -> Result<String, authenticator::Error> {
            let mut tokens = self.tokens.lock().unwrap();
            if tokens.len() > 1 {
                return Ok(tokens.remove(0));
            }
            tokens
                .first()
                .cloned()
                .ok_or_else(|| authenticator::Error::AuthenticationFailed {
                    description: "no token".to_string(),
                })
        }
    }
    #[async_trait::async_trait]
    impl Authenticator for FakeAuthenticator {
        async fn access_token(&self) -> Result<String, authenticator::Error> {
            self.calls.lock().unwrap().push(AuthCall::Access);
            self.next_token()
        }

        async fn refreshed_access_token(&self) -> Result<String, authenticator::Error> {
            self.calls.lock().unwrap().push(AuthCall::Refresh);
            if self.refresh_fails {
                return Err(authenticator::Error::AuthenticationFailed {
                    description: "refresh failed".to_string(),
                });
            }
            self.next_token()
        }
    }

    fn response(status_code: u16) -> Result<HttpResponse, transport::Error> {
        Ok(HttpResponse {
            status_code,
            body: Vec::new(),
        })
    }

    fn request(method: HttpMethod) -> HttpRequest {
        HttpRequest {
            method,
            url: "https://openapi.tidal.com/v2/installations".to_string(),
            headers: HashMap::new(),
            body: None,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn get_retries_transient_responses() {
        let transport = FakeTransport::new(vec![
            response(503),
            response(429),
            response(500),
            response(200),
        ]);
        let client = HttpClient::new(transport.clone(), None);

        let result = client.execute(request(HttpMethod::Get)).await.unwrap();

        assert_eq!(result.status_code, 200);
        assert_eq!(transport.request_count(), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn get_uses_the_timeout_retry_budget() {
        let results = (0..4)
            .map(|_| {
                Err(transport::Error::TimeoutError {
                    description: "timed out".to_string(),
                })
            })
            .collect();
        let transport = FakeTransport::new(results);
        let client = HttpClient::new(transport.clone(), None);

        let result = client.execute(request(HttpMethod::Get)).await;

        assert!(matches!(
            result,
            Err(Error::Transport(transport::Error::TimeoutError { .. }))
        ));
        assert_eq!(transport.request_count(), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn get_uses_the_network_retry_budget() {
        let results = (0..11)
            .map(|_| {
                Err(transport::Error::NetworkError {
                    description: "offline".to_string(),
                })
            })
            .collect();
        let transport = FakeTransport::new(results);
        let client = HttpClient::new(transport.clone(), None);

        let result = client.execute(request(HttpMethod::Get)).await;

        assert!(matches!(
            result,
            Err(Error::Transport(transport::Error::NetworkError { .. }))
        ));
        assert_eq!(transport.request_count(), 11);
    }

    #[tokio::test]
    async fn other_transport_errors_are_not_retried() {
        let transport = FakeTransport::new(vec![Err(transport::Error::OtherError {
            description: "bad URL".to_string(),
        })]);
        let client = HttpClient::new(transport.clone(), None);

        let result = client.execute(request(HttpMethod::Get)).await;

        assert!(matches!(
            result,
            Err(Error::Transport(transport::Error::OtherError { .. }))
        ));
        assert_eq!(transport.request_count(), 1);
    }

    #[tokio::test]
    async fn non_get_requests_are_not_retried() {
        let transport = FakeTransport::new(vec![response(503)]);
        let client = HttpClient::new(transport.clone(), None);

        let result = client.execute(request(HttpMethod::Post)).await.unwrap();

        assert_eq!(result.status_code, 503);
        assert_eq!(transport.request_count(), 1);
    }

    #[tokio::test]
    async fn unauthorized_refreshes_the_token_once() {
        let transport = FakeTransport::new(vec![response(401), response(200)]);
        let authenticator = FakeAuthenticator::new(&["stale", "fresh"]);
        let client = HttpClient::new(transport.clone(), Some(authenticator.clone()));

        let result = client.execute(request(HttpMethod::Get)).await.unwrap();

        assert_eq!(result.status_code, 200);
        let requests = transport.requests.lock().unwrap();
        assert_eq!(
            requests[0].headers.get("Authorization").map(String::as_str),
            Some("Bearer stale")
        );
        assert_eq!(
            requests[1].headers.get("Authorization").map(String::as_str),
            Some("Bearer fresh")
        );
        assert_eq!(
            *authenticator.calls.lock().unwrap(),
            vec![AuthCall::Access, AuthCall::Refresh]
        );
    }

    #[tokio::test]
    async fn a_second_unauthorized_response_is_returned_without_another_refresh() {
        let transport = FakeTransport::new(vec![response(401), response(401)]);
        let authenticator = FakeAuthenticator::new(&["stale", "fresh"]);
        let client = HttpClient::new(transport.clone(), Some(authenticator.clone()));

        let result = client.execute(request(HttpMethod::Get)).await.unwrap();

        assert_eq!(result.status_code, 401);
        assert_eq!(transport.request_count(), 2);
        assert_eq!(
            *authenticator.calls.lock().unwrap(),
            vec![AuthCall::Access, AuthCall::Refresh]
        );
    }

    #[tokio::test]
    async fn a_failed_refresh_returns_the_original_unauthorized_response() {
        let transport = FakeTransport::new(vec![response(401)]);
        let authenticator = FakeAuthenticator::with_failing_refresh("stale");
        let client = HttpClient::new(transport.clone(), Some(authenticator.clone()));

        let result = client.execute(request(HttpMethod::Get)).await.unwrap();

        assert_eq!(result.status_code, 401);
        assert_eq!(transport.request_count(), 1);
        assert_eq!(
            *authenticator.calls.lock().unwrap(),
            vec![AuthCall::Access, AuthCall::Refresh]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn authentication_refresh_does_not_consume_the_response_retry_budget() {
        let transport = FakeTransport::new(vec![
            response(401),
            response(503),
            response(503),
            response(503),
            response(200),
        ]);
        let authenticator =
            FakeAuthenticator::new(&["stale", "fresh", "token-c", "token-d", "token-e"]);
        let client = HttpClient::new(transport.clone(), Some(authenticator));

        let result = client.execute(request(HttpMethod::Get)).await.unwrap();

        assert_eq!(result.status_code, 200);
        assert_eq!(transport.request_count(), 5);
    }
}
