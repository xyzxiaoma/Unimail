use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use secrecy::{ExposeSecret, SecretBox};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as AsyncMutex;
use unimail_core::{
    Cancellation, CredentialRef, CredentialStore, ProviderError, ProviderErrorKind, ProviderResult,
};

use super::{
    client::GmailHttp,
    config::{GmailConfig, REQUIRED_SCOPES},
    dto::TokenResponse,
};

const ENVELOPE_VERSION: u8 = 1;
const REFRESH_SKEW_SECS: i64 = 120;

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct GmailCredentialEnvelopeV1 {
    pub(super) version: u8,
    pub(super) access_token: String,
    pub(super) refresh_token: String,
    pub(super) token_type: String,
    pub(super) expires_at_epoch_secs: i64,
    pub(super) scopes: Vec<String>,
}

impl GmailCredentialEnvelopeV1 {
    pub(super) fn from_token(
        token: TokenResponse,
        previous_refresh: Option<&str>,
    ) -> ProviderResult<Self> {
        if token.access_token.trim().is_empty()
            || !token.token_type.eq_ignore_ascii_case("bearer")
            || token.expires_in == 0
        {
            return Err(ProviderError::new(
                ProviderErrorKind::Protocol,
                "gmail_token_response_invalid",
            ));
        }
        let refresh_token = token
            .refresh_token
            .or_else(|| previous_refresh.map(ToOwned::to_owned))
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                ProviderError::new(
                    ProviderErrorKind::Authentication,
                    "gmail_refresh_token_missing",
                )
            })?;
        let expires_in = i64::try_from(token.expires_in).map_err(|_| {
            ProviderError::new(ProviderErrorKind::Protocol, "gmail_token_expiry_invalid")
        })?;
        let scopes: Vec<String> = if token.scope.trim().is_empty() {
            REQUIRED_SCOPES
                .iter()
                .map(|value| (*value).to_owned())
                .collect()
        } else {
            token
                .scope
                .split_whitespace()
                .map(ToOwned::to_owned)
                .collect()
        };
        if !REQUIRED_SCOPES
            .iter()
            .all(|required| scopes.iter().any(|scope| scope == required))
        {
            return Err(ProviderError::new(
                ProviderErrorKind::Permission,
                "gmail_required_scope_missing",
            ));
        }
        Ok(Self {
            version: ENVELOPE_VERSION,
            access_token: token.access_token,
            refresh_token,
            token_type: token.token_type,
            expires_at_epoch_secs: now_epoch_secs()?.saturating_add(expires_in),
            scopes,
        })
    }

    fn needs_refresh(&self) -> ProviderResult<bool> {
        Ok(self.expires_at_epoch_secs <= now_epoch_secs()?.saturating_add(REFRESH_SKEW_SECS))
    }
}

impl fmt::Debug for GmailCredentialEnvelopeV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GmailCredentialEnvelopeV1")
            .field("version", &self.version)
            .field("scope_count", &self.scopes.len())
            .finish_non_exhaustive()
    }
}

pub(super) struct GmailCredentialManager {
    store: Arc<dyn CredentialStore>,
    config: GmailConfig,
    http: GmailHttp,
    refresh_locks: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
}

impl GmailCredentialManager {
    pub(super) fn new(
        config: GmailConfig,
        store: Arc<dyn CredentialStore>,
        http: GmailHttp,
    ) -> Self {
        Self {
            store,
            config,
            http,
            refresh_locks: Mutex::new(HashMap::new()),
        }
    }

    pub(super) fn create_reference() -> CredentialRef {
        CredentialRef::new(format!("gmail-oauth-{}", uuid::Uuid::new_v4()))
    }

    pub(super) fn persist(
        &self,
        reference: &CredentialRef,
        envelope: &GmailCredentialEnvelopeV1,
    ) -> ProviderResult<()> {
        let bytes = serde_json::to_vec(envelope).map_err(|_| {
            ProviderError::new(
                ProviderErrorKind::Permanent,
                "gmail_credential_encode_failed",
            )
        })?;
        self.store
            .put(reference, SecretBox::new(bytes.into_boxed_slice()))
            .map_err(|_| {
                ProviderError::new(
                    ProviderErrorKind::Permanent,
                    "gmail_credential_write_failed",
                )
            })
    }

    pub(super) fn load(
        &self,
        reference: &CredentialRef,
    ) -> ProviderResult<GmailCredentialEnvelopeV1> {
        let secret = self.store.get(reference).map_err(|_| {
            ProviderError::new(
                ProviderErrorKind::Authentication,
                "gmail_credential_read_failed",
            )
        })?;
        let secret = secret.ok_or_else(|| {
            ProviderError::new(
                ProviderErrorKind::Authentication,
                "gmail_credential_missing",
            )
        })?;
        let envelope: GmailCredentialEnvelopeV1 = serde_json::from_slice(secret.expose_secret())
            .map_err(|_| {
                ProviderError::new(
                    ProviderErrorKind::Authentication,
                    "gmail_credential_invalid",
                )
            })?;
        if envelope.version != ENVELOPE_VERSION
            || envelope.access_token.is_empty()
            || envelope.refresh_token.is_empty()
            || !envelope.token_type.eq_ignore_ascii_case("bearer")
        {
            return Err(ProviderError::new(
                ProviderErrorKind::Authentication,
                "gmail_credential_invalid",
            ));
        }
        Ok(envelope)
    }

    pub(super) async fn access_token(
        &self,
        reference: &CredentialRef,
        force_refresh: bool,
        cancellation: &dyn Cancellation,
    ) -> ProviderResult<String> {
        let envelope = self.load(reference)?;
        if !force_refresh && !envelope.needs_refresh()? {
            return Ok(envelope.access_token);
        }

        let lock = self.refresh_lock(reference)?;
        let _guard = tokio::select! {
            () = cancellation.cancelled() => return Err(super::client::cancelled_error()),
            value = lock.lock() => value,
        };
        let current = self.load(reference)?;
        if current.access_token != envelope.access_token
            || current.refresh_token != envelope.refresh_token
            || current.expires_at_epoch_secs != envelope.expires_at_epoch_secs
        {
            return Ok(current.access_token);
        }
        if !force_refresh && !current.needs_refresh()? {
            return Ok(current.access_token);
        }
        let refreshed = self.refresh_token(&current, cancellation).await?;
        self.persist(reference, &refreshed)?;
        Ok(refreshed.access_token)
    }

    pub(super) fn delete(&self, reference: &CredentialRef) -> ProviderResult<()> {
        self.store.delete(reference).map_err(|_| {
            ProviderError::new(
                ProviderErrorKind::Permanent,
                "gmail_credential_delete_failed",
            )
        })
    }

    async fn refresh_token(
        &self,
        current: &GmailCredentialEnvelopeV1,
        cancellation: &dyn Cancellation,
    ) -> ProviderResult<GmailCredentialEnvelopeV1> {
        let client_id = self.config.require_client_id()?;
        let request = self
            .http
            .client()
            .post(&self.config.endpoints.token)
            .form(&[
                ("client_id", client_id),
                ("grant_type", "refresh_token"),
                ("refresh_token", current.refresh_token.as_str()),
            ]);
        let response = self
            .http
            .execute(request, cancellation)
            .await
            .map_err(super::client::DispatchError::into_provider)?;
        let token = self
            .http
            .json::<TokenResponse>(response, cancellation, false)
            .await?;
        GmailCredentialEnvelopeV1::from_token(token, Some(&current.refresh_token))
    }

    fn refresh_lock(&self, reference: &CredentialRef) -> ProviderResult<Arc<AsyncMutex<()>>> {
        let mut locks = self.refresh_locks.lock().map_err(|_| {
            ProviderError::new(
                ProviderErrorKind::Permanent,
                "gmail_refresh_lock_unavailable",
            )
        })?;
        Ok(locks
            .entry(reference.as_str().to_owned())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone())
    }
}

impl fmt::Debug for GmailCredentialManager {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("GmailCredentialManager([redacted])")
    }
}

fn now_epoch_secs() -> ProviderResult<i64> {
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| {
        ProviderError::new(ProviderErrorKind::Permanent, "gmail_system_time_invalid")
    })?;
    i64::try_from(duration.as_secs())
        .map_err(|_| ProviderError::new(ProviderErrorKind::Permanent, "gmail_system_time_invalid"))
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    use secrecy::{ExposeSecret, SecretBox};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
    };
    use unimail_core::{
        CredentialRef, CredentialStore, CredentialStoreError, CredentialStoreKind,
        ProviderErrorKind, SecretBytes,
    };

    use crate::fake::FakeCancellation;

    use super::{
        ENVELOPE_VERSION, GmailConfig, GmailCredentialEnvelopeV1, GmailCredentialManager,
        GmailHttp, REQUIRED_SCOPES, TokenResponse, now_epoch_secs,
    };

    #[derive(Default)]
    struct TestCredentialStore {
        values: Mutex<HashMap<String, Vec<u8>>>,
        fail_writes: Mutex<bool>,
    }

    impl TestCredentialStore {
        fn set_fail_writes(&self, fail: bool) {
            *self.fail_writes.lock().expect("write flag should lock") = fail;
        }

        fn bytes(&self, reference: &CredentialRef) -> Vec<u8> {
            self.values
                .lock()
                .expect("values should lock")
                .get(reference.as_str())
                .expect("credential should exist")
                .clone()
        }
    }

    impl CredentialStore for TestCredentialStore {
        fn kind(&self) -> CredentialStoreKind {
            CredentialStoreKind::Unsupported
        }

        fn get(
            &self,
            reference: &CredentialRef,
        ) -> Result<Option<SecretBytes>, CredentialStoreError> {
            Ok(self
                .values
                .lock()
                .map_err(|_| CredentialStoreError::OperationFailed)?
                .get(reference.as_str())
                .cloned()
                .map(|bytes| SecretBox::new(bytes.into_boxed_slice())))
        }

        fn put(
            &self,
            reference: &CredentialRef,
            value: SecretBytes,
        ) -> Result<(), CredentialStoreError> {
            if *self
                .fail_writes
                .lock()
                .map_err(|_| CredentialStoreError::OperationFailed)?
            {
                return Err(CredentialStoreError::OperationFailed);
            }
            self.values
                .lock()
                .map_err(|_| CredentialStoreError::OperationFailed)?
                .insert(
                    reference.as_str().to_owned(),
                    value.expose_secret().to_vec(),
                );
            Ok(())
        }

        fn delete(&self, reference: &CredentialRef) -> Result<(), CredentialStoreError> {
            self.values
                .lock()
                .map_err(|_| CredentialStoreError::OperationFailed)?
                .remove(reference.as_str());
            Ok(())
        }
    }

    fn envelope(
        access_token: &str,
        refresh_token: &str,
        expires_at: i64,
    ) -> GmailCredentialEnvelopeV1 {
        GmailCredentialEnvelopeV1 {
            version: ENVELOPE_VERSION,
            access_token: access_token.to_owned(),
            refresh_token: refresh_token.to_owned(),
            token_type: "Bearer".to_owned(),
            expires_at_epoch_secs: expires_at,
            scopes: REQUIRED_SCOPES
                .iter()
                .map(|scope| (*scope).to_owned())
                .collect(),
        }
    }

    async fn manager_with_response(
        store: Arc<TestCredentialStore>,
        status: &'static str,
        body: &'static str,
    ) -> (GmailCredentialManager, tokio::task::JoinHandle<String>) {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let address = listener.local_addr().expect("listener should have address");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("request should connect");
            let request = read_http_request(&mut stream).await;
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("response should write");
            String::from_utf8_lossy(&request).into_owned()
        });
        let config = GmailConfig::for_test(&format!("http://{address}"));
        let http = GmailHttp::new(config.clone()).expect("HTTP client should initialize");
        (GmailCredentialManager::new(config, store, http), server)
    }

    async fn read_http_request(stream: &mut TcpStream) -> Vec<u8> {
        let mut request = Vec::new();
        loop {
            let mut chunk = [0_u8; 1024];
            let read = stream.read(&mut chunk).await.expect("request should read");
            assert_ne!(read, 0, "request should include its declared body");
            request.extend_from_slice(&chunk[..read]);
            let Some(header_end) = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|position| position + 4)
            else {
                continue;
            };
            let header = std::str::from_utf8(&request[..header_end])
                .expect("request headers should be UTF-8");
            let content_length = header
                .lines()
                .find_map(|line| {
                    line.split_once(':').and_then(|(name, value)| {
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().expect("valid content length"))
                    })
                })
                .unwrap_or_default();
            if request.len() >= header_end + content_length {
                return request;
            }
        }
    }

    #[tokio::test]
    async fn envelope_round_trips_and_debug_is_redacted() {
        let store = Arc::new(TestCredentialStore::default());
        let config = GmailConfig::for_test("http://127.0.0.1:1");
        let manager = GmailCredentialManager::new(
            config.clone(),
            store.clone(),
            GmailHttp::new(config).expect("HTTP client should initialize"),
        );
        let reference = CredentialRef::new("gmail-oauth-fictional-reference");
        let expected = envelope("fake-access-secret", "fake-refresh-secret", i64::MAX);

        manager
            .persist(&reference, &expected)
            .expect("credential should persist");
        let decoded = manager.load(&reference).expect("credential should decode");
        let envelope_debug = format!("{decoded:?}");
        let manager_debug = format!("{manager:?}");

        assert_eq!(decoded.access_token, "fake-access-secret");
        assert_eq!(decoded.refresh_token, "fake-refresh-secret");
        assert!(!envelope_debug.contains("fake-access-secret"));
        assert!(!envelope_debug.contains("fake-refresh-secret"));
        assert!(!manager_debug.contains(reference.as_str()));
        assert_eq!(manager_debug, "GmailCredentialManager([redacted])");
    }

    #[tokio::test]
    async fn expired_credential_refreshes_and_rotates_refresh_token() {
        let store = Arc::new(TestCredentialStore::default());
        let (manager, server) = manager_with_response(
            store,
            "200 OK",
            r#"{"access_token":"new-access","expires_in":3600,"refresh_token":"new-refresh","scope":"https://www.googleapis.com/auth/gmail.modify https://www.googleapis.com/auth/gmail.send","token_type":"Bearer"}"#,
        )
        .await;
        let reference = CredentialRef::new("gmail-oauth-expired");
        manager
            .persist(&reference, &envelope("old-access", "old-refresh", 0))
            .expect("credential should persist");

        let access = manager
            .access_token(&reference, false, &FakeCancellation::default())
            .await
            .expect("expired credential should refresh");
        let refreshed = manager
            .load(&reference)
            .expect("refreshed credential should load");
        let request = server.await.expect("server should finish");

        assert_eq!(access, "new-access");
        assert_eq!(refreshed.refresh_token, "new-refresh");
        assert!(refreshed.expires_at_epoch_secs > now_epoch_secs().expect("time should work"));
        assert!(request.contains("grant_type=refresh_token"));
        assert!(request.contains("refresh_token=old-refresh"));
        assert!(!request.contains("client_secret"));
    }

    #[tokio::test]
    async fn refresh_retains_old_refresh_token_when_google_omits_one() {
        let store = Arc::new(TestCredentialStore::default());
        let (manager, server) = manager_with_response(
            store,
            "200 OK",
            r#"{"access_token":"new-access","expires_in":3600,"scope":"https://www.googleapis.com/auth/gmail.modify https://www.googleapis.com/auth/gmail.send","token_type":"Bearer"}"#,
        )
        .await;
        let reference = CredentialRef::new("gmail-oauth-retain");
        manager
            .persist(&reference, &envelope("old-access", "retained-refresh", 0))
            .expect("credential should persist");

        manager
            .access_token(&reference, false, &FakeCancellation::default())
            .await
            .expect("refresh should succeed");

        assert_eq!(
            manager
                .load(&reference)
                .expect("credential should load")
                .refresh_token,
            "retained-refresh"
        );
        server.await.expect("server should finish");
    }

    #[tokio::test]
    async fn refreshed_token_is_not_returned_when_credential_write_fails() {
        let store = Arc::new(TestCredentialStore::default());
        let (manager, server) = manager_with_response(
            store.clone(),
            "200 OK",
            r#"{"access_token":"unpersisted-access","expires_in":3600,"refresh_token":"unpersisted-refresh","scope":"https://www.googleapis.com/auth/gmail.modify https://www.googleapis.com/auth/gmail.send","token_type":"Bearer"}"#,
        )
        .await;
        let reference = CredentialRef::new("gmail-oauth-write-failure");
        manager
            .persist(&reference, &envelope("old-access", "old-refresh", 0))
            .expect("initial credential should persist");
        let original = store.bytes(&reference);
        store.set_fail_writes(true);

        let error = manager
            .access_token(&reference, false, &FakeCancellation::default())
            .await
            .expect_err("write failure must prevent success");

        assert_eq!(error.kind, ProviderErrorKind::Permanent);
        assert_eq!(error.code, "gmail_credential_write_failed");
        assert_eq!(store.bytes(&reference), original);
        server.await.expect("server should finish");
    }

    #[tokio::test]
    async fn concurrent_refresh_is_single_flight_even_when_access_token_is_reused() {
        let store = Arc::new(TestCredentialStore::default());
        let (manager, server) = manager_with_response(
            store,
            "200 OK",
            r#"{"access_token":"same-access","expires_in":3600,"scope":"https://www.googleapis.com/auth/gmail.modify https://www.googleapis.com/auth/gmail.send","token_type":"Bearer"}"#,
        )
        .await;
        let manager = Arc::new(manager);
        let reference = CredentialRef::new("gmail-oauth-single-flight");
        manager
            .persist(&reference, &envelope("same-access", "same-refresh", 0))
            .expect("credential should persist");
        let left_cancellation = FakeCancellation::default();
        let right_cancellation = FakeCancellation::default();

        let (left, right) = tokio::join!(
            manager.access_token(&reference, false, &left_cancellation),
            manager.access_token(&reference, false, &right_cancellation)
        );

        assert_eq!(left.expect("first refresh should succeed"), "same-access");
        assert_eq!(
            right.expect("second refresh should reuse result"),
            "same-access"
        );
        server.await.expect("one refresh request should finish");
    }

    #[tokio::test]
    async fn missing_credential_and_invalid_grant_are_authentication_failures() {
        let store = Arc::new(TestCredentialStore::default());
        let (manager, server) = manager_with_response(
            store,
            "400 Bad Request",
            r#"{"error":"invalid_grant","error_description":"fictional revoked token"}"#,
        )
        .await;
        let missing = CredentialRef::new("gmail-oauth-missing");
        let missing_error = manager
            .access_token(&missing, false, &FakeCancellation::default())
            .await
            .expect_err("missing credential should fail");
        assert_eq!(missing_error.kind, ProviderErrorKind::Authentication);
        assert_eq!(missing_error.code, "gmail_credential_missing");

        let revoked = CredentialRef::new("gmail-oauth-revoked");
        manager
            .persist(&revoked, &envelope("expired-access", "revoked-refresh", 0))
            .expect("credential should persist");
        let grant_error = manager
            .access_token(&revoked, false, &FakeCancellation::default())
            .await
            .expect_err("invalid grant should fail");

        assert_eq!(grant_error.kind, ProviderErrorKind::Authentication);
        assert_eq!(grant_error.code, "gmail_invalid_grant");
        assert!(!format!("{grant_error:?}").contains("fictional revoked token"));
        server.await.expect("server should finish");
    }

    #[test]
    fn token_envelope_requires_refresh_token_and_required_scopes() {
        let missing_refresh = GmailCredentialEnvelopeV1::from_token(
            TokenResponse {
                access_token: "fake-access".to_owned(),
                expires_in: 3600,
                refresh_token: None,
                scope: REQUIRED_SCOPES.join(" "),
                token_type: "Bearer".to_owned(),
            },
            None,
        )
        .expect_err("refresh token is required");
        assert_eq!(missing_refresh.code, "gmail_refresh_token_missing");

        let missing_scope = GmailCredentialEnvelopeV1::from_token(
            TokenResponse {
                access_token: "fake-access".to_owned(),
                expires_in: 3600,
                refresh_token: Some("fake-refresh".to_owned()),
                scope: REQUIRED_SCOPES[0].to_owned(),
                token_type: "Bearer".to_owned(),
            },
            None,
        )
        .expect_err("both required scopes are required");
        assert_eq!(missing_scope.kind, ProviderErrorKind::Permission);
        assert_eq!(missing_scope.code, "gmail_required_scope_missing");

        for token in [
            TokenResponse {
                access_token: String::new(),
                expires_in: 3600,
                refresh_token: Some("fake-refresh".to_owned()),
                token_type: "Bearer".to_owned(),
                scope: REQUIRED_SCOPES.join(" "),
            },
            TokenResponse {
                access_token: "fake-access".to_owned(),
                expires_in: 0,
                refresh_token: Some("fake-refresh".to_owned()),
                token_type: "Bearer".to_owned(),
                scope: REQUIRED_SCOPES.join(" "),
            },
            TokenResponse {
                access_token: "fake-access".to_owned(),
                expires_in: 3600,
                refresh_token: Some("fake-refresh".to_owned()),
                token_type: "MAC".to_owned(),
                scope: REQUIRED_SCOPES.join(" "),
            },
        ] {
            let error = GmailCredentialEnvelopeV1::from_token(token, None)
                .expect_err("malformed token response should fail");
            assert_eq!(error.kind, ProviderErrorKind::Protocol);
            assert_eq!(error.code, "gmail_token_response_invalid");
        }
    }
}
