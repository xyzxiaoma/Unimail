use std::{fmt, time::Duration};

use reqwest::{RequestBuilder, Response, StatusCode, header::RETRY_AFTER};
use serde::de::DeserializeOwned;
use unimail_core::{
    Cancellation, ProviderError, ProviderErrorKind, ProviderResult, RetryHint, SafeRequestId,
};

use super::{config::GmailConfig, dto::GoogleErrorEnvelope};

#[derive(Clone)]
pub(super) struct GmailHttp {
    client: reqwest::Client,
    config: GmailConfig,
}

impl GmailHttp {
    pub(super) fn new(config: GmailConfig) -> ProviderResult<Self> {
        let client = reqwest::Client::builder()
            .connect_timeout(config.connect_timeout)
            .timeout(config.request_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("Unimail/0.1 Gmail")
            .build()
            .map_err(|_| {
                ProviderError::new(ProviderErrorKind::Permanent, "gmail_http_init_failed")
            })?;
        Ok(Self { client, config })
    }

    pub(super) const fn client(&self) -> &reqwest::Client {
        &self.client
    }

    pub(super) async fn execute(
        &self,
        request: RequestBuilder,
        cancellation: &dyn Cancellation,
    ) -> Result<Response, DispatchError> {
        if cancellation.is_cancelled() {
            return Err(DispatchError::Cancelled);
        }
        tokio::select! {
            () = cancellation.cancelled() => Err(DispatchError::Cancelled),
            response = request.send() => response.map_err(|_| DispatchError::Transport),
        }
    }

    pub(super) async fn json<T: DeserializeOwned>(
        &self,
        response: Response,
        cancellation: &dyn Cancellation,
        history_cursor: bool,
    ) -> ProviderResult<T> {
        self.json_with_limit(
            response,
            cancellation,
            history_cursor,
            self.config.max_json_bytes,
        )
        .await
    }

    pub(super) async fn json_with_limit<T: DeserializeOwned>(
        &self,
        response: Response,
        cancellation: &dyn Cancellation,
        history_cursor: bool,
        limit: usize,
    ) -> ProviderResult<T> {
        let status = response.status();
        let request_id = safe_request_id(&response);
        let retry_after = retry_after(&response);
        if response
            .content_length()
            .is_some_and(|size| size > limit as u64)
        {
            return Err(with_request_id(
                ProviderError::new(ProviderErrorKind::Protocol, "gmail_response_too_large"),
                request_id,
            ));
        }
        let bytes = tokio::select! {
            () = cancellation.cancelled() => return Err(cancelled_error()),
            value = response.bytes() => value.map_err(|_| transport_error())?,
        };
        if bytes.len() > limit {
            return Err(with_request_id(
                ProviderError::new(ProviderErrorKind::Protocol, "gmail_response_too_large"),
                request_id,
            ));
        }
        if !status.is_success() {
            return Err(map_http_error(
                status,
                &bytes,
                retry_after,
                history_cursor,
                request_id,
            ));
        }
        serde_json::from_slice(&bytes).map_err(|_| {
            with_request_id(
                ProviderError::new(ProviderErrorKind::Protocol, "gmail_malformed_response"),
                request_id,
            )
        })
    }

    pub(super) async fn ensure_success(
        &self,
        response: Response,
        cancellation: &dyn Cancellation,
    ) -> ProviderResult<()> {
        let status = response.status();
        let request_id = safe_request_id(&response);
        let retry_after = retry_after(&response);
        let bytes = tokio::select! {
            () = cancellation.cancelled() => return Err(cancelled_error()),
            value = response.bytes() => value.map_err(|_| transport_error())?,
        };
        if status.is_success() {
            Ok(())
        } else {
            Err(map_http_error(
                status,
                &bytes,
                retry_after,
                false,
                request_id,
            ))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DispatchError {
    Cancelled,
    Transport,
}

impl DispatchError {
    pub(super) const fn into_provider(self) -> ProviderError {
        match self {
            Self::Cancelled => cancelled_error(),
            Self::Transport => transport_error(),
        }
    }
}

pub(super) fn map_http_error(
    status: StatusCode,
    body: &[u8],
    retry_after_value: Option<Duration>,
    history_cursor: bool,
    request_id: Option<SafeRequestId>,
) -> ProviderError {
    let error = if status == StatusCode::UNAUTHORIZED {
        ProviderError::new(
            ProviderErrorKind::Authentication,
            "gmail_authentication_required",
        )
    } else if history_cursor && status == StatusCode::NOT_FOUND {
        ProviderError::new(
            ProviderErrorKind::InvalidCursor,
            "gmail_history_cursor_invalid",
        )
    } else if status == StatusCode::TOO_MANY_REQUESTS {
        retryable_error(
            ProviderErrorKind::Throttled,
            "gmail_rate_limited",
            retry_after_value,
        )
    } else if status == StatusCode::FORBIDDEN {
        if is_retryable_quota(body) {
            retryable_error(
                ProviderErrorKind::Throttled,
                "gmail_quota_exceeded",
                retry_after_value,
            )
        } else {
            ProviderError::new(ProviderErrorKind::Permission, "gmail_permission_denied")
        }
    } else if matches!(status.as_u16(), 500..=504) {
        retryable_error(
            ProviderErrorKind::Transient,
            "gmail_temporarily_unavailable",
            retry_after_value,
        )
    } else if status == StatusCode::BAD_REQUEST && is_invalid_grant(body) {
        ProviderError::new(ProviderErrorKind::Authentication, "gmail_invalid_grant")
    } else if status.is_client_error() {
        ProviderError::new(ProviderErrorKind::Permanent, "gmail_request_rejected")
    } else {
        ProviderError::new(ProviderErrorKind::Protocol, "gmail_unexpected_status")
    };
    with_request_id(error, request_id)
}

fn retryable_error(
    kind: ProviderErrorKind,
    code: &'static str,
    retry_after_value: Option<Duration>,
) -> ProviderError {
    ProviderError::new(kind, code)
        .with_retry(retry_after_value.map_or(RetryHint::Backoff, RetryHint::After))
}

fn is_retryable_quota(body: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<GoogleErrorEnvelope>(body) else {
        return false;
    };
    value.error.errors.iter().any(|error| {
        error.reason.as_deref().is_some_and(|reason| {
            matches!(
                reason,
                "rateLimitExceeded" | "userRateLimitExceeded" | "quotaExceeded" | "backendError"
            )
        })
    })
}

fn is_invalid_grant(body: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
        return false;
    };
    value.get("error").is_some_and(|error| {
        error.as_str() == Some("invalid_grant")
            || error.get("error").and_then(serde_json::Value::as_str) == Some("invalid_grant")
    })
}

fn retry_after(response: &Response) -> Option<Duration> {
    response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
}

fn safe_request_id(response: &Response) -> Option<SafeRequestId> {
    ["x-google-request-id", "x-guploader-uploadid"]
        .iter()
        .filter_map(|name| response.headers().get(*name))
        .filter_map(|value| value.to_str().ok())
        .find(|value| {
            !value.is_empty()
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
        .map(SafeRequestId::new)
}

fn with_request_id(mut error: ProviderError, request_id: Option<SafeRequestId>) -> ProviderError {
    error.request_id = request_id;
    error
}

pub(super) const fn cancelled_error() -> ProviderError {
    ProviderError::new(ProviderErrorKind::Cancelled, "operation_cancelled")
}

pub(super) const fn transport_error() -> ProviderError {
    ProviderError::new(ProviderErrorKind::Transient, "gmail_transport_failed")
        .with_retry(RetryHint::Backoff)
}

impl fmt::Debug for GmailHttp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("GmailHttp([configured])")
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    use unimail_core::{ProviderErrorKind, RetryHint};

    use crate::fake::FakeCancellation;

    use super::{GmailConfig, GmailHttp};

    #[tokio::test]
    async fn json_error_preserves_retry_after() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let address = listener.local_addr().expect("listener should have address");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("request should connect");
            let mut request = [0_u8; 1024];
            let read = stream
                .read(&mut request)
                .await
                .expect("request should read");
            assert_ne!(read, 0, "request should contain headers");
            stream
                .write_all(
                    b"HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nRetry-After: 37\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                )
                .await
                .expect("response should write");
        });
        let http = GmailHttp::new(GmailConfig::for_test(&format!("http://{address}")))
            .expect("HTTP client should initialize");
        let response = http
            .client()
            .get(format!("http://{address}/gmail/v1/test"))
            .send()
            .await
            .expect("response should arrive");

        let error = http
            .json::<serde_json::Value>(response, &FakeCancellation::default(), false)
            .await
            .expect_err("429 should fail");

        assert_eq!(error.kind, ProviderErrorKind::Throttled);
        assert_eq!(error.retry, RetryHint::After(Duration::from_secs(37)));
        server.await.expect("server task should finish");
    }

    #[tokio::test]
    async fn custom_json_limit_rejects_oversized_attachment_envelopes() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let address = listener.local_addr().expect("listener should have address");
        let body = r#"{"data":"fictional-base64url-data"}"#;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("request should connect");
            let mut request = [0_u8; 1024];
            let read = stream
                .read(&mut request)
                .await
                .expect("request should read");
            assert_ne!(read, 0, "request should contain headers");
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("response should write");
        });
        let http = GmailHttp::new(GmailConfig::for_test(&format!("http://{address}")))
            .expect("HTTP client should initialize");
        let response = http
            .client()
            .get(format!("http://{address}/gmail/v1/test"))
            .send()
            .await
            .expect("response should arrive");

        let error = http
            .json_with_limit::<serde_json::Value>(response, &FakeCancellation::default(), false, 8)
            .await
            .expect_err("custom response limit should be enforced");

        assert_eq!(error.kind, ProviderErrorKind::Protocol);
        assert_eq!(error.code, "gmail_response_too_large");
        server.await.expect("server task should finish");
    }
}
