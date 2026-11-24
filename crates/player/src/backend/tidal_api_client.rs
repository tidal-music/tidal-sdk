use std::collections::HashMap;

use url::{Position, Url};

use super::http_client::{self, HttpClient};
use crate::platforms::transport::{HttpMethod, HttpRequest};

pub(crate) struct TidalApiClient {
    http_client: HttpClient,
    base_url: String,
}

impl TidalApiClient {
    pub(crate) fn new(http_client: HttpClient, base_url: String) -> Self {
        Self {
            http_client,
            base_url,
        }
    }

    #[allow(dead_code)]
    pub(crate) async fn execute_resources_request<T: serde::Serialize>(
        &self,
        method: HttpMethod,
        resource_type: &str,
        query: &[(&str, String)],
        body: Option<&T>,
    ) -> Result<TidalApiBody, Error> {
        self.execute(method, &[resource_type], query, body).await
    }

    pub(crate) async fn execute_resource_request<T: serde::Serialize>(
        &self,
        method: HttpMethod,
        resource_type: &str,
        resource_id: &str,
        query: &[(&str, String)],
        body: Option<&T>,
    ) -> Result<TidalApiBody, Error> {
        self.execute(method, &[resource_type, resource_id], query, body)
            .await
    }

    #[allow(dead_code)]
    pub(crate) async fn execute_resource_relationship_request<T: serde::Serialize>(
        &self,
        method: HttpMethod,
        resource_type: &str,
        resource_id: &str,
        relationship: &str,
        query: &[(&str, String)],
        body: Option<&T>,
    ) -> Result<TidalApiBody, Error> {
        self.execute(
            method,
            &[resource_type, resource_id, "relationships", relationship],
            query,
            body,
        )
        .await
    }

    async fn execute<T: serde::Serialize>(
        &self,
        method: HttpMethod,
        segments: &[&str],
        query: &[(&str, String)],
        body: Option<&T>,
    ) -> Result<TidalApiBody, Error> {
        let url = build_url(&self.base_url, segments, query)?;
        let path = url[Position::BeforePath..Position::AfterQuery].to_owned();
        let body = body
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|source| Error::Encode {
                method: method.as_str(),
                path: path.clone(),
                source,
            })?;

        let mut headers = HashMap::new();
        if body.is_some() {
            headers.insert(
                "Content-Type".to_string(),
                "application/vnd.api+json".to_string(),
            );
        }

        let request = HttpRequest {
            method,
            url: url.to_string(),
            headers,
            body,
        };

        let response = self.http_client.execute(request).await?;
        if !(200..300).contains(&response.status_code) {
            return Err(Error::UnsuccessfulResponse {
                http_status: response.status_code,
                error_document: serde_json::from_slice(&response.body).unwrap_or_default(),
            });
        }

        Ok(TidalApiBody {
            method: method.as_str(),
            path,
            body: response.body,
        })
    }
}

pub(crate) const NO_BODY: Option<&()> = None;

pub(crate) struct TidalApiBody {
    method: &'static str,
    path: String,
    body: Vec<u8>,
}

impl TidalApiBody {
    pub(crate) fn decode<T: serde::de::DeserializeOwned>(&self) -> Result<T, Error> {
        serde_json::from_slice(&self.body).map_err(|source| Error::Decode {
            method: self.method,
            path: self.path.clone(),
            source,
        })
    }
}

fn build_url(base_url: &str, segments: &[&str], query: &[(&str, String)]) -> Result<Url, Error> {
    let mut url =
        Url::parse(base_url.trim_end_matches('/')).map_err(|source| Error::InvalidBaseUrl {
            base_url: base_url.to_owned(),
            source: Some(source),
        })?;

    {
        let mut path_segments = url.path_segments_mut().map_err(|_| Error::InvalidBaseUrl {
            base_url: base_url.to_owned(),
            source: None,
        })?;

        path_segments.pop_if_empty();
        for segment in segments {
            path_segments.push(segment);
        }
    }

    if !query.is_empty() {
        let mut pairs = url.query_pairs_mut();
        for (name, value) in query {
            pairs.append_pair(name, value);
        }
    }

    Ok(url)
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unsuccessful HTTP response ({http_status}): {error_document:?}")]
    UnsuccessfulResponse {
        http_status: u16,
        error_document: ErrorDocument,
    },
    #[error(transparent)]
    HttpClient(#[from] http_client::Error),
    #[error("{method} {path} body encode failed: {source}")]
    Encode {
        method: &'static str,
        path: String,
        source: serde_json::Error,
    },
    #[error("{method} {path} response decode failed: {source}")]
    Decode {
        method: &'static str,
        path: String,
        source: serde_json::Error,
    },
    #[error("invalid base URL {base_url}")]
    InvalidBaseUrl {
        base_url: String,
        source: Option<url::ParseError>,
    },
}

#[derive(Debug, Default, serde::Deserialize)]
pub struct ErrorDocument {
    pub errors: Vec<ErrorObject>,
}

#[derive(Debug, serde::Deserialize)]
pub struct ErrorObject {
    pub code: Option<String>,
}
