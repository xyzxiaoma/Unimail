use std::{
    fmt,
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Notify,
};
use unimail_core::{Cancellation, CancellationFuture, SensitiveString};
use url::Url;

pub(crate) const CALLBACK_PATH: &str = "/oauth/callback";
pub(crate) const FLOW_TIMEOUT: Duration = Duration::from_mins(5);
const MAX_HTTP_REQUEST_BYTES: usize = 8 * 1024;
const READ_CHUNK_BYTES: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RedirectHost {
    Ipv4Loopback,
    Localhost,
}

impl RedirectHost {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Ipv4Loopback => "127.0.0.1",
            Self::Localhost => "localhost",
        }
    }
}

const COMPLETE_PAGE: &str = r#"<!doctype html>
<html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'">
<meta name="referrer" content="no-referrer"><title>Unimail 授权完成</title>
<style>body{font-family:system-ui,sans-serif;margin:0;background:#f5f7fb;color:#172033;display:grid;min-height:100vh;place-items:center}.card{background:#fff;border:1px solid #dfe5ef;border-radius:16px;padding:32px;max-width:420px;box-shadow:0 12px 40px #17203314}h1{font-size:22px;margin:0 0 12px}p{line-height:1.7;margin:0}</style></head>
<body><main class="card"><h1>授权信息已收到</h1><p>请返回 Unimail 查看连接结果。现在可以关闭此页面。</p></main></body></html>"#;

const ERROR_PAGE: &str = r#"<!doctype html>
<html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'">
<meta name="referrer" content="no-referrer"><title>Unimail 授权未完成</title>
<style>body{font-family:system-ui,sans-serif;margin:0;background:#f5f7fb;color:#172033;display:grid;min-height:100vh;place-items:center}.card{background:#fff;border:1px solid #dfe5ef;border-radius:16px;padding:32px;max-width:420px;box-shadow:0 12px 40px #17203314}h1{font-size:22px;margin:0 0 12px}p{line-height:1.7;margin:0}</style></head>
<body><main class="card"><h1>授权未完成</h1><p>请返回 Unimail 重试，或关闭此页面。</p></main></body></html>"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoopbackError {
    Bind,
    Accept,
    Cancelled,
    Timeout,
    InvalidRequest,
    OversizedRequest,
    WrongMethod,
    WrongPath,
    WrongState,
    WriteResponse,
}

impl fmt::Display for LoopbackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Bind => "loopback callback could not bind",
            Self::Accept => "loopback callback could not accept a connection",
            Self::Cancelled => "loopback callback was cancelled",
            Self::Timeout => "loopback callback timed out",
            Self::InvalidRequest => "loopback callback request was invalid",
            Self::OversizedRequest => "loopback callback request exceeded its limit",
            Self::WrongMethod => "loopback callback method was rejected",
            Self::WrongPath => "loopback callback path was rejected",
            Self::WrongState => "loopback callback state was rejected",
            Self::WriteResponse => "loopback callback response could not be written",
        })
    }
}

impl std::error::Error for LoopbackError {}

#[derive(Default)]
pub(crate) struct DesktopCancellation {
    cancelled: AtomicBool,
    notify: Notify,
}

impl DesktopCancellation {
    pub(crate) fn cancel(&self) {
        if !self.cancelled.swap(true, Ordering::AcqRel) {
            self.notify.notify_waiters();
        }
    }

    #[must_use]
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

impl Cancellation for DesktopCancellation {
    fn is_cancelled(&self) -> bool {
        self.is_cancelled()
    }

    fn cancelled(&self) -> CancellationFuture<'_> {
        Box::pin(async move {
            loop {
                let notified = self.notify.notified();
                if self.is_cancelled() {
                    return;
                }
                notified.await;
            }
        })
    }
}

pub(crate) trait BrowserOpener: Send + Sync {
    fn open(&self, authorization_url: &SensitiveString) -> Result<(), BrowserOpenError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BrowserOpenError;

pub(crate) struct SystemBrowserOpener;

impl BrowserOpener for SystemBrowserOpener {
    fn open(&self, authorization_url: &SensitiveString) -> Result<(), BrowserOpenError> {
        open_system_browser(authorization_url.expose())
    }
}

#[cfg(target_os = "windows")]
fn open_system_browser(url: &str) -> Result<(), BrowserOpenError> {
    use std::os::windows::process::CommandExt as _;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut command = Command::new("rundll32.exe");
    command
        .creation_flags(CREATE_NO_WINDOW)
        .arg("url.dll,FileProtocolHandler")
        .arg(url);
    command.spawn().map(|_| ()).map_err(|_| BrowserOpenError)
}

#[cfg(target_os = "macos")]
fn open_system_browser(url: &str) -> Result<(), BrowserOpenError> {
    Command::new("/usr/bin/open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|_| BrowserOpenError)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn open_system_browser(url: &str) -> Result<(), BrowserOpenError> {
    Command::new("xdg-open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|_| BrowserOpenError)
}

pub(crate) struct LoopbackReceiver {
    listener: TcpListener,
    port: u16,
    redirect_host: RedirectHost,
}

impl LoopbackReceiver {
    #[cfg(test)]
    pub(crate) async fn bind() -> Result<Self, LoopbackError> {
        Self::bind_for(RedirectHost::Ipv4Loopback).await
    }

    pub(crate) async fn bind_for(redirect_host: RedirectHost) -> Result<Self, LoopbackError> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| LoopbackError::Bind)?;
        let address = listener.local_addr().map_err(|_| LoopbackError::Bind)?;
        if address.ip() != std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST) {
            return Err(LoopbackError::Bind);
        }
        Ok(Self {
            listener,
            port: address.port(),
            redirect_host,
        })
    }

    #[must_use]
    pub(crate) fn redirect_uri(&self) -> SensitiveString {
        SensitiveString::new(format!(
            "http://{}:{}{CALLBACK_PATH}",
            self.redirect_host.as_str(),
            self.port
        ))
    }

    #[cfg(test)]
    const fn port(&self) -> u16 {
        self.port
    }

    pub(crate) async fn receive(
        self,
        expected_state: &str,
        cancellation: &dyn Cancellation,
        timeout: Duration,
    ) -> Result<SensitiveString, LoopbackError> {
        let receive = async {
            let (stream, peer) = tokio::select! {
                result = self.listener.accept() => result.map_err(|_| LoopbackError::Accept)?,
                () = cancellation.cancelled() => return Err(LoopbackError::Cancelled),
            };
            if peer.ip() != std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST) {
                return Err(LoopbackError::InvalidRequest);
            }
            handle_callback(
                stream,
                self.port,
                self.redirect_host,
                expected_state,
                cancellation,
            )
            .await
        };

        match tokio::time::timeout(timeout, receive).await {
            Ok(result) => result,
            Err(_) => Err(LoopbackError::Timeout),
        }
    }
}

async fn handle_callback(
    mut stream: TcpStream,
    port: u16,
    redirect_host: RedirectHost,
    expected_state: &str,
    cancellation: &dyn Cancellation,
) -> Result<SensitiveString, LoopbackError> {
    let parsed = read_and_validate_request(
        &mut stream,
        port,
        redirect_host,
        expected_state,
        cancellation,
    )
    .await;
    let (status, page) = if parsed.is_ok() {
        ("200 OK", COMPLETE_PAGE)
    } else {
        ("400 Bad Request", ERROR_PAGE)
    };
    write_page(&mut stream, status, page)
        .await
        .map_err(|_| LoopbackError::WriteResponse)?;
    parsed
}

async fn read_and_validate_request(
    stream: &mut TcpStream,
    port: u16,
    redirect_host: RedirectHost,
    expected_state: &str,
    cancellation: &dyn Cancellation,
) -> Result<SensitiveString, LoopbackError> {
    let header = read_request_header(stream, cancellation).await?;
    let mut lines = header.split("\r\n");
    let request_line = lines.next().ok_or(LoopbackError::InvalidRequest)?;
    let mut request_parts = request_line.split(' ');
    let method = request_parts.next().ok_or(LoopbackError::InvalidRequest)?;
    let target = request_parts.next().ok_or(LoopbackError::InvalidRequest)?;
    let version = request_parts.next().ok_or(LoopbackError::InvalidRequest)?;
    if request_parts.next().is_some() || !matches!(version, "HTTP/1.0" | "HTTP/1.1") {
        return Err(LoopbackError::InvalidRequest);
    }
    if method != "GET" {
        return Err(LoopbackError::WrongMethod);
    }
    if !target.starts_with('/') || target.contains('#') {
        return Err(LoopbackError::InvalidRequest);
    }

    validate_headers(lines, version, port, redirect_host)?;
    validate_callback_target(target, port, redirect_host, expected_state)
}

async fn read_request_header(
    stream: &mut TcpStream,
    cancellation: &dyn Cancellation,
) -> Result<String, LoopbackError> {
    let mut request = Vec::with_capacity(READ_CHUNK_BYTES);
    loop {
        if request.len() >= MAX_HTTP_REQUEST_BYTES {
            return Err(LoopbackError::OversizedRequest);
        }
        let remaining = MAX_HTTP_REQUEST_BYTES - request.len();
        let mut chunk = [0_u8; READ_CHUNK_BYTES];
        let read = tokio::select! {
            result = stream.read(&mut chunk[..remaining.min(READ_CHUNK_BYTES)]) => {
                result.map_err(|_| LoopbackError::InvalidRequest)?
            }
            () = cancellation.cancelled() => return Err(LoopbackError::Cancelled),
        };
        if read == 0 {
            return Err(LoopbackError::InvalidRequest);
        }
        request.extend_from_slice(&chunk[..read]);
        if find_header_end(&request).is_some() {
            break;
        }
    }
    let header_end = find_header_end(&request).ok_or(LoopbackError::InvalidRequest)?;
    if request.len() > header_end {
        return Err(LoopbackError::InvalidRequest);
    }
    String::from_utf8(request[..header_end].to_vec()).map_err(|_| LoopbackError::InvalidRequest)
}

fn validate_headers<'a>(
    lines: impl Iterator<Item = &'a str>,
    version: &str,
    port: u16,
    redirect_host: RedirectHost,
) -> Result<(), LoopbackError> {
    let mut content_length = None;
    let mut transfer_encoding = false;
    let mut host = None;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line.split_once(':').ok_or(LoopbackError::InvalidRequest)?;
        if !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(LoopbackError::InvalidRequest);
        }
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(LoopbackError::InvalidRequest);
            }
            content_length = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| LoopbackError::InvalidRequest)?,
            );
        } else if name.eq_ignore_ascii_case("host") {
            if host.is_some() {
                return Err(LoopbackError::InvalidRequest);
            }
            host = Some(value);
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            transfer_encoding = true;
        }
    }
    if content_length.unwrap_or(0) != 0 || transfer_encoding {
        return Err(LoopbackError::InvalidRequest);
    }
    let expected_host = format!("{}:{port}", redirect_host.as_str());
    if (version == "HTTP/1.1" && host.is_none()) || host.is_some_and(|host| host != expected_host) {
        return Err(LoopbackError::InvalidRequest);
    }
    Ok(())
}

fn validate_callback_target(
    target: &str,
    port: u16,
    redirect_host: RedirectHost,
    expected_state: &str,
) -> Result<SensitiveString, LoopbackError> {
    let parsed = Url::parse(&format!("http://{}:{port}{target}", redirect_host.as_str()))
        .map_err(|_| LoopbackError::InvalidRequest)?;
    if parsed.path() != CALLBACK_PATH {
        return Err(LoopbackError::WrongPath);
    }
    let states = parsed
        .query_pairs()
        .filter_map(|(key, value)| (key == "state").then_some(value.into_owned()))
        .collect::<Vec<_>>();
    if states.len() != 1
        || states[0].len() != expected_state.len()
        || states[0]
            .as_bytes()
            .ct_eq(expected_state.as_bytes())
            .unwrap_u8()
            != 1
    {
        return Err(LoopbackError::WrongState);
    }
    let results = parsed
        .query_pairs()
        .filter(|(key, value)| matches!(key.as_ref(), "code" | "error") && !value.is_empty())
        .count();
    if results != 1 {
        return Err(LoopbackError::InvalidRequest);
    }
    Ok(SensitiveString::new(parsed.to_string()))
}

fn find_header_end(request: &[u8]) -> Option<usize> {
    request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4)
}

async fn write_page(stream: &mut TcpStream, status: &str, body: &str) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nPragma: no-cache\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

#[must_use]
pub(crate) fn oauth_state_from_authorization_url(
    authorization_url: &SensitiveString,
) -> Option<String> {
    let parsed = Url::parse(authorization_url.expose()).ok()?;
    let states = parsed
        .query_pairs()
        .filter_map(|(key, value)| (key == "state").then_some(value.into_owned()))
        .collect::<Vec<_>>();
    (states.len() == 1 && !states[0].is_empty()).then(|| states[0].clone())
}

#[cfg(test)]
mod tests {
    use std::{io::ErrorKind, time::Duration};

    use tokio::{io::AsyncReadExt, net::TcpStream};
    use unimail_core::SensitiveString;

    use super::{
        CALLBACK_PATH, COMPLETE_PAGE, DesktopCancellation, ERROR_PAGE, LoopbackError,
        LoopbackReceiver, RedirectHost, oauth_state_from_authorization_url,
    };

    async fn exchange(port: u16, request: Vec<u8>) -> String {
        let mut stream = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .expect("connect callback");
        tokio::io::AsyncWriteExt::write_all(&mut stream, &request)
            .await
            .expect("write callback");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .await
            .expect("read callback page");
        response
    }

    fn request(port: u16, method: &str, target: &str) -> Vec<u8> {
        request_with_host(port, method, target, "127.0.0.1")
    }

    fn request_with_host(port: u16, method: &str, target: &str, host: &str) -> Vec<u8> {
        format!("{method} {target} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n")
            .into_bytes()
    }

    #[tokio::test]
    async fn accepts_one_exact_callback_and_returns_secret_free_chinese_page() {
        let receiver = LoopbackReceiver::bind().await.expect("bind callback");
        let port = receiver.port();
        assert_eq!(
            receiver.redirect_uri().expose(),
            format!("http://127.0.0.1:{port}{CALLBACK_PATH}")
        );
        let cancellation = DesktopCancellation::default();
        let client = tokio::spawn(exchange(
            port,
            request(
                port,
                "GET",
                "/oauth/callback?code=fake-code&state=fake-state",
            ),
        ));
        let callback = receiver
            .receive("fake-state", &cancellation, Duration::from_secs(1))
            .await
            .expect("valid callback");
        assert_eq!(
            callback.expose(),
            format!("http://127.0.0.1:{port}/oauth/callback?code=fake-code&state=fake-state")
        );
        let response = client.await.expect("client task");
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("授权信息已收到"));
        assert!(!response.contains("fake-code"));
        assert!(!response.contains("fake-state"));
        assert!(response.contains("Content-Security-Policy"));
        assert!(response.contains("Referrer-Policy: no-referrer"));
    }

    #[tokio::test]
    async fn outlook_exposes_localhost_redirect_while_binding_ipv4_loopback() {
        let receiver = LoopbackReceiver::bind_for(RedirectHost::Localhost)
            .await
            .expect("bind Outlook callback");
        let port = receiver.port();
        assert_eq!(
            receiver.redirect_uri().expose(),
            format!("http://localhost:{port}{CALLBACK_PATH}")
        );
        let cancellation = DesktopCancellation::default();
        let client = tokio::spawn(exchange(
            port,
            request_with_host(
                port,
                "GET",
                "/oauth/callback?code=fake-code&state=fake-state",
                "localhost",
            ),
        ));
        let callback = receiver
            .receive("fake-state", &cancellation, Duration::from_secs(1))
            .await
            .expect("valid Outlook callback");
        assert_eq!(
            callback.expose(),
            format!("http://localhost:{port}/oauth/callback?code=fake-code&state=fake-state")
        );
        assert!(
            client
                .await
                .expect("client task")
                .starts_with("HTTP/1.1 200 OK")
        );
    }

    #[tokio::test]
    async fn rejects_wrong_method_path_state_and_missing_result() {
        for (method, target, expected) in [
            (
                "POST",
                "/oauth/callback?code=fake&state=expected",
                LoopbackError::WrongMethod,
            ),
            (
                "GET",
                "/favicon.ico?code=fake&state=expected",
                LoopbackError::WrongPath,
            ),
            (
                "GET",
                "/oauth/callback?code=fake&state=wrong",
                LoopbackError::WrongState,
            ),
            (
                "GET",
                "/oauth/callback?state=expected",
                LoopbackError::InvalidRequest,
            ),
        ] {
            let receiver = LoopbackReceiver::bind().await.expect("bind callback");
            let port = receiver.port();
            let cancellation = DesktopCancellation::default();
            let client = tokio::spawn(exchange(port, request(port, method, target)));
            let result = receiver
                .receive("expected", &cancellation, Duration::from_secs(1))
                .await;
            assert!(matches!(result, Err(error) if error == expected));
            let response = client.await.expect("client task");
            assert!(response.starts_with("HTTP/1.1 400 Bad Request"));
            assert!(response.contains("授权未完成"));
        }
    }

    #[tokio::test]
    async fn rejects_oversized_request_without_echoing_it() {
        let receiver = LoopbackReceiver::bind().await.expect("bind callback");
        let port = receiver.port();
        let cancellation = DesktopCancellation::default();
        let oversized = format!(
            "GET /oauth/callback?code=fake&state=expected HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Fill: {}\r\n\r\n",
            "x".repeat(9 * 1024)
        )
        .into_bytes();
        let client = tokio::spawn(async move {
            let mut stream = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
                .await
                .expect("connect callback");
            tokio::io::AsyncWriteExt::write_all(&mut stream, &oversized)
                .await
                .expect("write callback");
            let mut response = String::new();
            let read_result = stream.read_to_string(&mut response).await;
            (response, read_result)
        });
        let result = receiver
            .receive("expected", &cancellation, Duration::from_secs(1))
            .await;
        assert!(matches!(result, Err(LoopbackError::OversizedRequest)));
        let (response, read_result) = client.await.expect("client task");
        if let Err(error) = read_result {
            assert_eq!(error.kind(), ErrorKind::ConnectionReset);
        }
        assert!(response.is_empty() || response.starts_with("HTTP/1.1 400 Bad Request"));
        assert!(!response.contains("X-Fill"));
    }

    #[tokio::test]
    async fn timeout_and_cancellation_end_the_listener() {
        let receiver = LoopbackReceiver::bind().await.expect("bind callback");
        let result = receiver
            .receive(
                "expected",
                &DesktopCancellation::default(),
                Duration::from_millis(10),
            )
            .await;
        assert!(matches!(result, Err(LoopbackError::Timeout)));

        let receiver = LoopbackReceiver::bind().await.expect("bind callback");
        let cancellation = std::sync::Arc::new(DesktopCancellation::default());
        let cancel = std::sync::Arc::clone(&cancellation);
        tokio::spawn(async move { cancel.cancel() });
        let result = receiver
            .receive("expected", cancellation.as_ref(), Duration::from_secs(1))
            .await;
        assert!(matches!(result, Err(LoopbackError::Cancelled)));
    }

    #[test]
    fn extracts_exactly_one_non_empty_state_and_pages_are_static() {
        assert_eq!(
            oauth_state_from_authorization_url(&SensitiveString::new(
                "https://accounts.example.test/auth?client_id=fake&state=fake-state"
            )),
            Some("fake-state".to_owned())
        );
        assert!(
            oauth_state_from_authorization_url(&SensitiveString::new(
                "https://accounts.example.test/auth?state=one&state=two"
            ))
            .is_none()
        );
        for page in [COMPLETE_PAGE, ERROR_PAGE] {
            assert!(page.contains("lang=\"zh-CN\""));
            assert!(page.contains("default-src 'none'"));
            assert!(!page.contains("{{"));
            assert!(!page.contains("fake-token"));
        }
    }
}
