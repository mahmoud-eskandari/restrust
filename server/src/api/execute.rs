//! Authenticated request execution for the same-origin web client.
use std::time::{Duration, Instant};

use axum::{extract::State, Json};
use restrust_core::execution::{ExecuteRequest, ExecuteResponse};

use crate::{auth::AuthUser, error::ApiError, state::SharedState};

const MAX_BODY: usize = 10 * 1024 * 1024;

pub async fn execute(
    _user: AuthUser,
    State(state): State<SharedState>,
    Json(input): Json<ExecuteRequest>,
) -> Result<Json<ExecuteResponse>, ApiError> {
    let url = reqwest::Url::parse(input.url.trim())
        .map_err(|_| ApiError::bad_request("Enter a valid HTTP or HTTPS URL"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(ApiError::bad_request(
            "Only HTTP and HTTPS URLs are supported",
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ApiError::bad_request(
            "Use an Authorization header instead of URL credentials",
        ));
    }
    if input.body.len() > MAX_BODY {
        return Err(ApiError::bad_request(
            "Request body exceeds the 10 MiB limit",
        ));
    }
    let method = reqwest::Method::from_bytes(input.method.as_bytes())
        .map_err(|_| ApiError::bad_request("Invalid HTTP method"))?;
    if !matches!(
        method.as_str(),
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS"
    ) {
        return Err(ApiError::bad_request("Unsupported HTTP method"));
    }
    let mut headers = reqwest::header::HeaderMap::new();
    for (name, value) in input.headers {
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| ApiError::bad_request("Invalid request header name"))?;
        // Framing and hop-by-hop headers belong to this outbound connection.
        if matches!(
            name.as_str(),
            "host"
                | "content-length"
                | "connection"
                | "transfer-encoding"
                | "upgrade"
                | "te"
                | "trailer"
                | "keep-alive"
                | "proxy-authorization"
                | "proxy-connection"
        ) {
            continue;
        }
        let value = reqwest::header::HeaderValue::from_str(&value)
            .map_err(|_| ApiError::bad_request("Invalid request header value"))?;
        headers.append(name, value);
    }
    let started = Instant::now();
    // Bound the entire exchange, including streaming the body.
    let exchange = async {
        let mut response = state
            .http
            .request(method, url)
            .headers(headers)
            .body(input.body)
            .send()
            .await
            .map_err(|err| ApiError::upstream(err.without_url().to_string()))?;
        let status = response.status();
        let url = response.url().to_string();
        let headers = response
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.to_string(),
                    String::from_utf8_lossy(v.as_bytes()).into_owned(),
                )
            })
            .collect();
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|err| ApiError::upstream(err.without_url().to_string()))?
        {
            if body.len() + chunk.len() > MAX_BODY {
                return Err(ApiError::upstream("Response body exceeds the 10 MiB limit"));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(Json(ExecuteResponse {
            url,
            status: status.as_u16(),
            reason: status.canonical_reason().unwrap_or("").into(),
            headers,
            body,
            duration_ms: started.elapsed().as_millis() as u64,
        }))
    };
    tokio::time::timeout(Duration::from_secs(30), exchange)
        .await
        .map_err(|_| ApiError::upstream("Target request timed out after 30 seconds"))?
}
