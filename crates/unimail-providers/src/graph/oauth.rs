use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use unimail_core::{
    AccountAuthenticator, AuthenticatedAccount, AuthorizationCodeLoginRequest, Cancellation,
    CompleteLoginRequest, CredentialRef, CredentialStore, LoginStart, Provider, ProviderError,
    ProviderErrorKind, ProviderFuture, ProviderResult, SensitiveString, StartLoginRequest,
};
use url::Url;

use super::{
    client::GraphHttp,
    config::{GraphConfig, REQUIRED_SCOPES},
    credential::{GraphCredentialEnvelopeV1, GraphCredentialManager},
    dto::{GraphProfile, TokenResponse},
};

const FLOW_TIMEOUT: Duration = Duration::from_mins(5);

struct OAuthFlow {
    state: SensitiveString,
    verifier: SensitiveString,
    redirect_uri: SensitiveString,
    deadline: Instant,
}

impl fmt::Debug for OAuthFlow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OAuthFlow")
            .field("state", &"[redacted]")
            .field("verifier", &"[redacted]")
            .field("redirect_uri", &"[redacted]")
            .finish_non_exhaustive()
    }
}

/// Graph Authorization Code + PKCE authenticator for a desktop-owned loopback callback.
pub struct GraphAuthenticator {
    config: GraphConfig,
    http: GraphHttp,
    credentials: GraphCredentialManager,
    flows: Mutex<HashMap<String, OAuthFlow>>,
}

impl GraphAuthenticator {
    /// Creates an authenticator using fixed Microsoft production endpoints.
    ///
    /// # Errors
    ///
    /// Returns a fixed provider error when the HTTP client cannot be initialized.
    pub fn new(
        config: GraphConfig,
        credential_store: Arc<dyn CredentialStore>,
    ) -> ProviderResult<Self> {
        let http = GraphHttp::new(config.clone())?;
        let credentials =
            GraphCredentialManager::new(config.clone(), credential_store, http.clone());
        Ok(Self {
            config,
            http,
            credentials,
            flows: Mutex::new(HashMap::new()),
        })
    }

    #[cfg(test)]
    pub(super) fn with_test_config(
        config: GraphConfig,
        credential_store: Arc<dyn CredentialStore>,
    ) -> ProviderResult<Self> {
        Self::new(config, credential_store)
    }

    /// Abandons an OAuth flow after desktop cancellation, timeout, or browser-open failure.
    ///
    /// # Errors
    ///
    /// Returns a fixed provider error if the flow registry is unavailable. Missing flows are
    /// treated as already abandoned.
    pub fn abandon_login(&self, flow_id: &str) -> ProviderResult<()> {
        self.lock_flows()?.remove(flow_id);
        Ok(())
    }

    fn start(&self, request: StartLoginRequest) -> ProviderResult<LoginStart> {
        if request.provider != Provider::Outlook {
            return Err(ProviderError::new(
                ProviderErrorKind::Permanent,
                "graph_provider_mismatch",
            ));
        }
        let client_id = self.config.require_client_id()?;
        validate_loopback_redirect(request.redirect_uri.expose())?;

        let flow_id = uuid::Uuid::new_v4().to_string();
        let state = random_urlsafe(32)?;
        let verifier = random_urlsafe(64)?;
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let mut authorization = Url::parse(&self.config.endpoints.authorization).map_err(|_| {
            ProviderError::new(ProviderErrorKind::Permanent, "graph_endpoint_invalid")
        })?;
        authorization
            .query_pairs_mut()
            .append_pair("client_id", client_id)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", request.redirect_uri.expose())
            .append_pair("scope", &REQUIRED_SCOPES.join(" "))
            .append_pair("response_mode", "query")
            .append_pair("prompt", "select_account")
            .append_pair("state", &state)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256");

        let mut flows = self.lock_flows()?;
        let now = Instant::now();
        flows.retain(|_, flow| flow.deadline >= now);
        flows.clear();
        flows.insert(
            flow_id.clone(),
            OAuthFlow {
                state: SensitiveString::new(state),
                verifier: SensitiveString::new(verifier),
                redirect_uri: request.redirect_uri,
                deadline: Instant::now() + FLOW_TIMEOUT,
            },
        );
        Ok(LoginStart {
            flow_id,
            authorization_url: SensitiveString::new(authorization.to_string()),
        })
    }

    async fn complete(
        &self,
        request: CompleteLoginRequest,
        cancellation: &dyn Cancellation,
    ) -> ProviderResult<AuthenticatedAccount> {
        let flow = self.lock_flows()?.remove(&request.flow_id).ok_or_else(|| {
            ProviderError::new(ProviderErrorKind::Permanent, "graph_oauth_flow_not_found")
        })?;
        if Instant::now() > flow.deadline {
            return Err(ProviderError::new(
                ProviderErrorKind::Permanent,
                "graph_oauth_flow_expired",
            ));
        }
        let callback = validate_callback(
            request.callback_url.expose(),
            flow.redirect_uri.expose(),
            flow.state.expose(),
        )?;
        let token = self
            .exchange_code(
                &callback,
                flow.redirect_uri.expose(),
                flow.verifier.expose(),
                cancellation,
            )
            .await?;
        let envelope = GraphCredentialEnvelopeV1::from_token(token, None)?;
        let profile = self.profile(&envelope.access_token, cancellation).await?;
        validate_profile(&profile)?;
        let reference = GraphCredentialManager::create_reference();
        self.credentials.persist(&reference, &envelope)?;
        Ok(authenticated_account(&profile, reference))
    }

    async fn exchange_code(
        &self,
        code: &str,
        redirect_uri: &str,
        verifier: &str,
        cancellation: &dyn Cancellation,
    ) -> ProviderResult<TokenResponse> {
        let client_id = self.config.require_client_id()?;
        let request = self
            .http
            .client()
            .post(&self.config.endpoints.token)
            .form(&[
                ("client_id", client_id),
                ("code", code),
                ("code_verifier", verifier),
                ("grant_type", "authorization_code"),
                ("redirect_uri", redirect_uri),
            ]);
        let response = self
            .http
            .execute(request, cancellation)
            .await
            .map_err(super::client::DispatchError::into_provider)?;
        self.http.json(response, cancellation, false).await
    }

    async fn profile(
        &self,
        access_token: &str,
        cancellation: &dyn Cancellation,
    ) -> ProviderResult<GraphProfile> {
        let url = format!(
            "{}/me?$select=displayName,mail,userPrincipalName",
            self.config.endpoints.api
        );
        let request = self.http.client().get(url).bearer_auth(access_token);
        let response = self
            .http
            .execute(request, cancellation)
            .await
            .map_err(super::client::DispatchError::into_provider)?;
        self.http.json(response, cancellation, false).await
    }

    fn lock_flows(&self) -> ProviderResult<MutexGuard<'_, HashMap<String, OAuthFlow>>> {
        self.flows.lock().map_err(|_| {
            ProviderError::new(
                ProviderErrorKind::Permanent,
                "graph_oauth_state_unavailable",
            )
        })
    }
}

impl AccountAuthenticator for GraphAuthenticator {
    fn start_login<'a>(
        &'a self,
        request: StartLoginRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, LoginStart> {
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return Err(super::client::cancelled_error());
            }
            self.start(request)
        })
    }

    fn complete_login<'a>(
        &'a self,
        request: CompleteLoginRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, AuthenticatedAccount> {
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return Err(super::client::cancelled_error());
            }
            self.complete(request, cancellation).await
        })
    }

    fn connect_with_authorization_code<'a>(
        &'a self,
        _request: AuthorizationCodeLoginRequest,
        _cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, AuthenticatedAccount> {
        Box::pin(async {
            Err(ProviderError::new(
                ProviderErrorKind::Permanent,
                "graph_authorization_code_unsupported",
            ))
        })
    }

    fn refresh<'a>(
        &'a self,
        credential_ref: &'a CredentialRef,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, AuthenticatedAccount> {
        Box::pin(async move {
            let access_token = self
                .credentials
                .access_token(credential_ref, true, cancellation)
                .await?;
            let profile = self.profile(&access_token, cancellation).await?;
            validate_profile(&profile)?;
            Ok(authenticated_account(&profile, credential_ref.clone()))
        })
    }

    fn revoke<'a>(
        &'a self,
        credential_ref: &'a CredentialRef,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, ()> {
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return Err(super::client::cancelled_error());
            }
            self.credentials.delete(credential_ref)
        })
    }
}

impl fmt::Debug for GraphAuthenticator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GraphAuthenticator")
            .field("configured", &self.config.is_configured())
            .finish_non_exhaustive()
    }
}

fn validate_loopback_redirect(value: &str) -> ProviderResult<()> {
    let url = Url::parse(value).map_err(|_| invalid_redirect())?;
    let path_valid = url.path() == "/oauth/callback";
    let authority_valid =
        url.scheme() == "http" && url.host_str() == Some("localhost") && url.port().is_some();
    if authority_valid
        && path_valid
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
    {
        Ok(())
    } else {
        Err(invalid_redirect())
    }
}

fn validate_callback(value: &str, redirect: &str, expected_state: &str) -> ProviderResult<String> {
    let mut url = Url::parse(value).map_err(|_| invalid_callback())?;
    if url.fragment().is_some() {
        return Err(invalid_callback());
    }
    let mut code = None;
    let mut state = None;
    let mut provider_error = None;
    for (name, value) in url.query_pairs() {
        match name.as_ref() {
            "code" if code.is_none() => code = Some(value.into_owned()),
            "state" if state.is_none() => state = Some(value.into_owned()),
            "error" if provider_error.is_none() => provider_error = Some(value.into_owned()),
            "code" | "state" | "error" => return Err(invalid_callback()),
            _ => {}
        }
    }
    url.set_query(None);
    if url.as_str() != redirect {
        return Err(invalid_callback());
    }
    let state = state.ok_or_else(invalid_callback)?;
    if !constant_time_equal(state.as_bytes(), expected_state.as_bytes()) {
        return Err(ProviderError::new(
            ProviderErrorKind::Permanent,
            "graph_oauth_state_mismatch",
        ));
    }
    if provider_error.as_deref() == Some("access_denied") {
        return Err(ProviderError::new(
            ProviderErrorKind::Permanent,
            "graph_authorization_denied",
        ));
    }
    if provider_error.is_some() {
        return Err(ProviderError::new(
            ProviderErrorKind::Authentication,
            "graph_authorization_failed",
        ));
    }
    code.filter(|value| !value.is_empty())
        .ok_or_else(invalid_callback)
}

fn validate_profile(profile: &GraphProfile) -> ProviderResult<()> {
    if profile.account_address().is_none() {
        Err(ProviderError::new(
            ProviderErrorKind::Protocol,
            "graph_profile_invalid",
        ))
    } else {
        Ok(())
    }
}

fn authenticated_account(
    profile: &GraphProfile,
    credential_ref: CredentialRef,
) -> AuthenticatedAccount {
    AuthenticatedAccount {
        provider: Provider::Outlook,
        account_address: profile
            .account_address()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase(),
        display_name: profile
            .display_name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        credential_ref,
        capabilities: vec!["mail.read_write".to_owned(), "mail.send".to_owned()],
    }
}

fn random_urlsafe(byte_count: usize) -> ProviderResult<String> {
    let mut bytes = vec![0_u8; byte_count];
    getrandom::fill(&mut bytes).map_err(|_| {
        ProviderError::new(ProviderErrorKind::Permanent, "graph_random_unavailable")
    })?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    let maximum = left.len().max(right.len());
    for index in 0..maximum {
        let left_byte = left.get(index).copied().unwrap_or_default();
        let right_byte = right.get(index).copied().unwrap_or_default();
        difference |= usize::from(left_byte ^ right_byte);
    }
    difference == 0
}

const fn invalid_redirect() -> ProviderError {
    ProviderError::new(ProviderErrorKind::Permanent, "graph_redirect_invalid")
}

const fn invalid_callback() -> ProviderError {
    ProviderError::new(ProviderErrorKind::Permanent, "graph_callback_invalid")
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    use secrecy::ExposeSecret;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
    };
    use unimail_core::{CredentialStoreError, CredentialStoreKind, SecretBytes, StartLoginRequest};

    use super::*;

    struct EmptyCredentials;

    impl CredentialStore for EmptyCredentials {
        fn kind(&self) -> CredentialStoreKind {
            CredentialStoreKind::Unsupported
        }

        fn get(
            &self,
            _reference: &CredentialRef,
        ) -> Result<Option<SecretBytes>, CredentialStoreError> {
            Ok(None)
        }

        fn put(
            &self,
            _reference: &CredentialRef,
            _value: SecretBytes,
        ) -> Result<(), CredentialStoreError> {
            Ok(())
        }

        fn delete(&self, _reference: &CredentialRef) -> Result<(), CredentialStoreError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingCredentials {
        values: Mutex<HashMap<String, Vec<u8>>>,
    }

    impl RecordingCredentials {
        fn contains(&self, reference: &CredentialRef) -> bool {
            self.values
                .lock()
                .expect("credential values should lock")
                .contains_key(reference.as_str())
        }
    }

    impl CredentialStore for RecordingCredentials {
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
                .map(|bytes| secrecy::SecretBox::new(bytes.into_boxed_slice())))
        }

        fn put(
            &self,
            reference: &CredentialRef,
            value: SecretBytes,
        ) -> Result<(), CredentialStoreError> {
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

    fn authenticator() -> GraphAuthenticator {
        let _ = rustls::crypto::ring::default_provider().install_default();
        GraphAuthenticator::with_test_config(
            GraphConfig::for_test("http://127.0.0.1:9"),
            Arc::new(EmptyCredentials),
        )
        .expect("test authenticator")
    }

    async fn read_http_request(stream: &mut TcpStream) -> String {
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
                return String::from_utf8(request).expect("request should be UTF-8");
            }
        }
    }

    async fn write_json(stream: &mut TcpStream, body: &str) {
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream
            .write_all(response.as_bytes())
            .await
            .expect("response should write");
    }

    #[test]
    fn start_uses_exact_desktop_pkce_contract_and_replaces_the_previous_flow() {
        let authenticator = authenticator();
        let redirect = "http://localhost:43127/oauth/callback";
        let first = authenticator
            .start(StartLoginRequest {
                provider: Provider::Outlook,
                redirect_uri: SensitiveString::new(redirect),
            })
            .expect("start first flow");
        let authorization = Url::parse(first.authorization_url.expose()).expect("authorization");
        let query = authorization
            .query_pairs()
            .into_owned()
            .collect::<HashMap<_, _>>();
        let expected_scope = REQUIRED_SCOPES.join(" ");

        assert_eq!(authorization.path(), "/common/oauth2/v2.0/authorize");
        assert_eq!(
            query.get("redirect_uri").map(String::as_str),
            Some(redirect)
        );
        assert_eq!(query.get("response_type").map(String::as_str), Some("code"));
        assert_eq!(
            query.get("scope").map(String::as_str),
            Some(expected_scope.as_str())
        );
        assert_eq!(
            query.get("response_mode").map(String::as_str),
            Some("query")
        );
        assert_eq!(
            query.get("prompt").map(String::as_str),
            Some("select_account")
        );
        assert_eq!(
            query.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        assert!(query.get("state").is_some_and(|value| !value.is_empty()));
        assert!(
            query
                .get("code_challenge")
                .is_some_and(|value| !value.is_empty())
        );
        assert!(!query.contains_key("client_secret"));

        let second = authenticator
            .start(StartLoginRequest {
                provider: Provider::Outlook,
                redirect_uri: SensitiveString::new(redirect),
            })
            .expect("start replacement flow");
        let flows = authenticator.lock_flows().expect("flow registry");
        assert_eq!(flows.len(), 1);
        assert!(!flows.contains_key(&first.flow_id));
        assert!(flows.contains_key(&second.flow_id));
    }

    #[test]
    fn redirect_and_callback_validation_rejects_unsafe_or_replayed_values() {
        assert!(validate_loopback_redirect("http://localhost:43127/oauth/callback").is_ok());
        assert!(validate_loopback_redirect("http://127.0.0.1:43127/oauth/callback").is_err());
        assert!(validate_loopback_redirect("http://localhost:43127/other").is_err());
        assert_eq!(
            validate_callback(
                "http://localhost:43127/oauth/callback?code=fake&state=wrong",
                "http://localhost:43127/oauth/callback",
                "expected",
            )
            .expect_err("state mismatch")
            .code,
            "graph_oauth_state_mismatch"
        );
        assert_eq!(
            validate_callback(
                "http://localhost:43127/oauth/callback?error=access_denied&state=expected",
                "http://localhost:43127/oauth/callback",
                "expected",
            )
            .expect_err("authorization denial")
            .code,
            "graph_authorization_denied"
        );
    }

    #[test]
    fn profile_falls_back_to_user_principal_name() {
        let profile = GraphProfile {
            display_name: Some("Fictional User".to_owned()),
            mail: None,
            user_principal_name: Some("User@Tenant.Example".to_owned()),
        };
        validate_profile(&profile).expect("UPN should be a valid account address");
        let account = authenticated_account(&profile, CredentialRef::new("outlook-oauth-test"));
        assert_eq!(account.account_address, "user@tenant.example");
        assert_eq!(account.display_name.as_deref(), Some("Fictional User"));
    }

    #[tokio::test]
    async fn complete_exchanges_code_profiles_and_persists_backend_credentials() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let address = listener.local_addr().expect("listener should have address");
        let server = tokio::spawn(async move {
            let (mut token_stream, _) = listener.accept().await.expect("token request");
            let token_request = read_http_request(&mut token_stream).await;
            write_json(
                &mut token_stream,
                r#"{"access_token":"fake-access","expires_in":3600,"refresh_token":"fake-refresh","scope":"offline_access User.Read Mail.ReadWrite Mail.Send","token_type":"Bearer"}"#,
            )
            .await;

            let (mut profile_stream, _) = listener.accept().await.expect("profile request");
            let profile_request = read_http_request(&mut profile_stream).await;
            write_json(
                &mut profile_stream,
                r#"{"displayName":"Fictional Owner","mail":"Owner@Example.Test","userPrincipalName":"owner@tenant.example.test"}"#,
            )
            .await;
            (token_request, profile_request)
        });
        let credentials = Arc::new(RecordingCredentials::default());
        let authenticator = GraphAuthenticator::with_test_config(
            GraphConfig::for_test(&format!("http://{address}")),
            credentials.clone(),
        )
        .expect("test authenticator");
        let redirect = "http://localhost:43127/oauth/callback";
        let login = authenticator
            .start(StartLoginRequest {
                provider: Provider::Outlook,
                redirect_uri: SensitiveString::new(redirect),
            })
            .expect("flow should start");
        let authorization = Url::parse(login.authorization_url.expose()).expect("authorization");
        let state = authorization
            .query_pairs()
            .find_map(|(key, value)| (key == "state").then(|| value.into_owned()))
            .expect("state should exist");

        let account = authenticator
            .complete(
                CompleteLoginRequest {
                    flow_id: login.flow_id,
                    callback_url: SensitiveString::new(format!(
                        "{redirect}?code=fake-code&state={state}"
                    )),
                },
                &crate::fake::FakeCancellation::default(),
            )
            .await
            .expect("OAuth completion should succeed");
        let (token_request, profile_request) = server.await.expect("server should finish");

        assert_eq!(account.provider, Provider::Outlook);
        assert_eq!(account.account_address, "owner@example.test");
        assert!(credentials.contains(&account.credential_ref));
        assert!(token_request.starts_with("POST /common/oauth2/v2.0/token HTTP/1.1"));
        assert!(token_request.contains("code=fake-code"));
        assert!(token_request.contains("code_verifier="));
        assert!(token_request.contains("grant_type=authorization_code"));
        assert!(
            token_request
                .contains("redirect_uri=http%3A%2F%2Flocalhost%3A43127%2Foauth%2Fcallback")
        );
        assert!(!token_request.contains("client_secret"));
        assert!(profile_request.starts_with("GET /v1.0/me?"));
        assert!(profile_request.contains("select=displayName"));
        assert!(profile_request.contains("mail"));
        assert!(profile_request.contains("userPrincipalName"));
        assert!(
            profile_request
                .to_ascii_lowercase()
                .contains("authorization: bearer fake-access")
        );
    }

    #[tokio::test]
    async fn revoke_deletes_backend_credential_locally() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let credentials = Arc::new(RecordingCredentials::default());
        let authenticator = GraphAuthenticator::with_test_config(
            GraphConfig::for_test("http://127.0.0.1:9"),
            credentials.clone(),
        )
        .expect("test authenticator");
        let reference = CredentialRef::new("graph-oauth-revoke-test");
        authenticator
            .credentials
            .persist(
                &reference,
                &GraphCredentialEnvelopeV1 {
                    version: 1,
                    access_token: "fake-access".to_owned(),
                    refresh_token: "fake-refresh".to_owned(),
                    token_type: "Bearer".to_owned(),
                    expires_at_epoch_secs: i64::MAX,
                    scopes: REQUIRED_SCOPES
                        .iter()
                        .map(|scope| (*scope).to_owned())
                        .collect(),
                },
            )
            .expect("credential should persist");

        let cancellation = crate::fake::FakeCancellation::default();
        AccountAuthenticator::revoke(&authenticator, &reference, &cancellation)
            .await
            .expect("revocation should succeed");
        assert!(!credentials.contains(&reference));
    }
}
