use std::{
    collections::HashSet,
    future::Future,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    pin::Pin,
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use reqwest::{StatusCode, header};
use scraper::{Html, Selector};
use unimail_core::{RemoteImageResultV1, StorageCommandError, StorageErrorCode};
use url::{Host, Url};

const MAX_REMOTE_IMAGE_URL_LENGTH: usize = 2_048;
pub(crate) const MAX_REMOTE_IMAGES_PER_MESSAGE: usize = 12;
const MAX_REMOTE_IMAGE_BYTES: usize = 2 * 1_024 * 1_024;
const MAX_REMOTE_IMAGE_DIMENSION: usize = 8_192;
const MAX_REMOTE_IMAGE_PIXELS: usize = 32 * 1_024 * 1_024;
const MAX_REDIRECTS: usize = 3;

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteImageError {
    Invalid,
    Unavailable,
}

impl RemoteImageError {
    fn into_command(self) -> StorageCommandError {
        let code = match self {
            Self::Invalid => StorageErrorCode::InvalidData,
            Self::Unavailable => StorageErrorCode::Internal,
        };
        StorageCommandError::from_code(code)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RemoteImageResponse {
    status: StatusCode,
    location: Option<String>,
    media_type: Option<String>,
    body: Vec<u8>,
}

trait RemoteImageResolver: Send + Sync {
    fn resolve<'a>(
        &'a self,
        host: &'a str,
        port: u16,
    ) -> BoxFuture<'a, Result<Vec<IpAddr>, RemoteImageError>>;
}

trait RemoteImageTransport: Send + Sync {
    fn get<'a>(
        &'a self,
        url: &'a Url,
        pinned_address: SocketAddr,
    ) -> BoxFuture<'a, Result<RemoteImageResponse, RemoteImageError>>;
}

struct SystemResolver;

impl RemoteImageResolver for SystemResolver {
    fn resolve<'a>(
        &'a self,
        host: &'a str,
        port: u16,
    ) -> BoxFuture<'a, Result<Vec<IpAddr>, RemoteImageError>> {
        Box::pin(async move {
            let addresses = tokio::net::lookup_host((host, port))
                .await
                .map_err(|_| RemoteImageError::Unavailable)?;
            Ok(addresses.map(|address| address.ip()).collect())
        })
    }
}

struct ReqwestTransport;

impl RemoteImageTransport for ReqwestTransport {
    fn get<'a>(
        &'a self,
        url: &'a Url,
        pinned_address: SocketAddr,
    ) -> BoxFuture<'a, Result<RemoteImageResponse, RemoteImageError>> {
        Box::pin(async move {
            let _ = rustls::crypto::ring::default_provider().install_default();
            let host = url.host_str().ok_or(RemoteImageError::Invalid)?;
            let client = reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(12))
                .redirect(reqwest::redirect::Policy::none())
                .resolve(host, pinned_address)
                .user_agent("Unimail/0.1 remote-image")
                .build()
                .map_err(|_| RemoteImageError::Unavailable)?;
            let request = build_request(&client, url);
            let mut response = request
                .send()
                .await
                .map_err(|_| RemoteImageError::Unavailable)?;
            let status = response.status();
            let location = response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let media_type = response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .and_then(normalize_media_type);
            if is_redirect(status) {
                return Ok(RemoteImageResponse {
                    status,
                    location,
                    media_type,
                    body: Vec::new(),
                });
            }
            if response
                .content_length()
                .is_some_and(|length| length > MAX_REMOTE_IMAGE_BYTES as u64)
            {
                return Err(RemoteImageError::Invalid);
            }
            let mut body = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| RemoteImageError::Unavailable)?
            {
                if body.len().saturating_add(chunk.len()) > MAX_REMOTE_IMAGE_BYTES {
                    return Err(RemoteImageError::Invalid);
                }
                body.extend_from_slice(&chunk);
            }
            Ok(RemoteImageResponse {
                status,
                location,
                media_type,
                body,
            })
        })
    }
}

fn build_request(client: &reqwest::Client, url: &Url) -> reqwest::RequestBuilder {
    client.get(url.clone()).header(
        header::ACCEPT,
        "image/png,image/jpeg,image/gif,image/webp;q=0.9,*/*;q=0.1",
    )
}

pub(crate) async fn fetch_remote_image(
    html: &str,
    requested_url: &str,
) -> Result<RemoteImageResultV1, StorageCommandError> {
    let normalized =
        validate_remote_image_url(requested_url).map_err(RemoteImageError::into_command)?;
    if !extract_remote_image_urls(html).contains(normalized.as_str()) {
        return Err(RemoteImageError::Invalid.into_command());
    }
    fetch_with(&normalized, &SystemResolver, &ReqwestTransport)
        .await
        .map_err(RemoteImageError::into_command)
}

async fn fetch_with(
    initial_url: &Url,
    resolver: &dyn RemoteImageResolver,
    transport: &dyn RemoteImageTransport,
) -> Result<RemoteImageResultV1, RemoteImageError> {
    let mut url = initial_url.clone();
    for redirect_count in 0..=MAX_REDIRECTS {
        let pinned_address = resolve_public_address(&url, resolver).await?;
        let response = transport.get(&url, pinned_address).await?;
        if is_redirect(response.status) {
            if redirect_count == MAX_REDIRECTS {
                return Err(RemoteImageError::Invalid);
            }
            let location = response.location.ok_or(RemoteImageError::Invalid)?;
            let redirected = url.join(&location).map_err(|_| RemoteImageError::Invalid)?;
            url = validate_remote_image_url(redirected.as_str())?;
            continue;
        }
        if !response.status.is_success() {
            return Err(RemoteImageError::Unavailable);
        }
        return validate_image_response(response.media_type.as_deref(), &response.body);
    }
    Err(RemoteImageError::Invalid)
}

async fn resolve_public_address(
    url: &Url,
    resolver: &dyn RemoteImageResolver,
) -> Result<SocketAddr, RemoteImageError> {
    let port = url
        .port_or_known_default()
        .ok_or(RemoteImageError::Invalid)?;
    let addresses = match url.host().ok_or(RemoteImageError::Invalid)? {
        Host::Ipv4(address) => vec![IpAddr::V4(address)],
        Host::Ipv6(address) => vec![IpAddr::V6(address)],
        Host::Domain(host) => resolver.resolve(host, port).await?,
    };
    if addresses.is_empty() || addresses.iter().any(|address| !is_public_ip(*address)) {
        return Err(RemoteImageError::Invalid);
    }
    Ok(SocketAddr::new(addresses[0], port))
}

fn validate_remote_image_url(value: &str) -> Result<Url, RemoteImageError> {
    if value.len() > MAX_REMOTE_IMAGE_URL_LENGTH || value.chars().any(char::is_control) {
        return Err(RemoteImageError::Invalid);
    }
    let url = Url::parse(value).map_err(|_| RemoteImageError::Invalid)?;
    if url.scheme() != "https"
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port_or_known_default() != Some(443)
        || url.fragment().is_some()
    {
        return Err(RemoteImageError::Invalid);
    }
    Ok(url)
}

fn extract_remote_image_urls(html: &str) -> HashSet<String> {
    let document = Html::parse_fragment(html);
    let selector = Selector::parse("img[src]").expect("static image selector should be valid");
    let mut urls = HashSet::new();
    for source in document
        .select(&selector)
        .filter_map(|image| image.value().attr("src"))
    {
        if urls.len() == MAX_REMOTE_IMAGES_PER_MESSAGE {
            break;
        }
        if let Ok(url) = validate_remote_image_url(source) {
            urls.insert(url.to_string());
        }
    }
    urls
}

fn normalize_media_type(value: &str) -> Option<String> {
    let media_type = value.split(';').next()?.trim().to_ascii_lowercase();
    matches!(
        media_type.as_str(),
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    )
    .then_some(media_type)
}

fn validate_image_response(
    declared_media_type: Option<&str>,
    body: &[u8],
) -> Result<RemoteImageResultV1, RemoteImageError> {
    if body.is_empty() || body.len() > MAX_REMOTE_IMAGE_BYTES {
        return Err(RemoteImageError::Invalid);
    }
    let declared = declared_media_type
        .and_then(normalize_media_type)
        .ok_or(RemoteImageError::Invalid)?;
    let detected = detect_media_type(body).ok_or(RemoteImageError::Invalid)?;
    if declared != detected {
        return Err(RemoteImageError::Invalid);
    }
    let dimensions = imagesize::blob_size(body).map_err(|_| RemoteImageError::Invalid)?;
    if dimensions.width == 0
        || dimensions.height == 0
        || dimensions.width > MAX_REMOTE_IMAGE_DIMENSION
        || dimensions.height > MAX_REMOTE_IMAGE_DIMENSION
        || dimensions.width.saturating_mul(dimensions.height) > MAX_REMOTE_IMAGE_PIXELS
    {
        return Err(RemoteImageError::Invalid);
    }
    Ok(RemoteImageResultV1 {
        media_type: declared.clone(),
        data_url: format!("data:{declared};base64,{}", STANDARD.encode(body)),
    })
}

fn detect_media_type(body: &[u8]) -> Option<String> {
    if body.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png".to_owned())
    } else if body.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg".to_owned())
    } else if body.starts_with(b"GIF87a") || body.starts_with(b"GIF89a") {
        Some("image/gif".to_owned())
    } else if body.len() >= 12 && body.starts_with(b"RIFF") && &body[8..12] == b"WEBP" {
        Some("image/webp".to_owned())
    } else {
        None
    }
}

fn is_redirect(status: StatusCode) -> bool {
    matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308)
}

fn is_public_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_public_ipv4(address),
        IpAddr::V6(address) => is_public_ipv6(address),
    }
}

fn is_public_ipv4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    !(a == 0
        || a == 10
        || a == 127
        || a >= 224
        || (a == 100 && (b & 0b1100_0000) == 64)
        || (a == 169 && b == 254)
        || (a == 172 && (b & 0b1111_0000) == 16)
        || (a == 192 && b == 0 && matches!(c, 0 | 2))
        || (a == 192 && b == 168)
        || (a == 198 && matches!(b, 18 | 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113))
}

fn is_public_ipv6(address: Ipv6Addr) -> bool {
    if let Some(address) = address.to_ipv4() {
        return is_public_ipv4(address);
    }
    let segments = address.segments();
    (segments[0] & 0xe000) == 0x2000
        && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
        && !(segments[0] == 0x2001 && segments[1] == 0)
        && segments[0] != 0x2002
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{HashMap, VecDeque},
        sync::Mutex,
    };

    use super::*;

    const ONE_PIXEL_PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 4,
        0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 252, 255, 31, 0, 3, 3,
        2, 0, 239, 191, 104, 111, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];

    struct FakeResolver {
        answers: HashMap<String, Vec<IpAddr>>,
    }

    impl RemoteImageResolver for FakeResolver {
        fn resolve<'a>(
            &'a self,
            host: &'a str,
            _port: u16,
        ) -> BoxFuture<'a, Result<Vec<IpAddr>, RemoteImageError>> {
            Box::pin(async move {
                self.answers
                    .get(host)
                    .cloned()
                    .ok_or(RemoteImageError::Unavailable)
            })
        }
    }

    struct FakeTransport {
        responses: Mutex<VecDeque<RemoteImageResponse>>,
        calls: Mutex<Vec<(String, SocketAddr)>>,
    }

    impl RemoteImageTransport for FakeTransport {
        fn get<'a>(
            &'a self,
            url: &'a Url,
            pinned_address: SocketAddr,
        ) -> BoxFuture<'a, Result<RemoteImageResponse, RemoteImageError>> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .expect("calls lock")
                    .push((url.to_string(), pinned_address));
                self.responses
                    .lock()
                    .expect("responses lock")
                    .pop_front()
                    .ok_or(RemoteImageError::Unavailable)
            })
        }
    }

    fn response(
        status: StatusCode,
        location: Option<&str>,
        media_type: Option<&str>,
        body: &[u8],
    ) -> RemoteImageResponse {
        RemoteImageResponse {
            status,
            location: location.map(str::to_owned),
            media_type: media_type.map(str::to_owned),
            body: body.to_vec(),
        }
    }

    #[test]
    fn message_manifest_keeps_only_normalized_https_images() {
        let urls = extract_remote_image_urls(
            r#"<img src="https://images.example.test/a.png"><img src="https://images.example.test/fragment.png#pixel"><img src="http://images.example.test/b.png"><script><img src="https://evil.example.test/x.png"></script>"#,
        );
        assert_eq!(
            urls,
            HashSet::from(["https://images.example.test/a.png".to_owned()])
        );
    }

    #[test]
    fn url_and_address_policy_rejects_credentialed_or_non_public_targets() {
        assert!(validate_remote_image_url("https://images.example.test/a.png").is_ok());
        for invalid in [
            "http://images.example.test/a.png",
            "https://user:secret@images.example.test/a.png",
            "https://images.example.test:444/a.png",
            "https://images.example.test/a.png\nnext",
        ] {
            assert_eq!(
                validate_remote_image_url(invalid),
                Err(RemoteImageError::Invalid)
            );
        }
        for private in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.1.1",
            "172.16.0.1",
            "192.168.0.1",
            "100.64.0.1",
            "224.0.0.1",
        ] {
            let address = private.parse().expect("test IPv4 address");
            assert!(!is_public_ip(IpAddr::V4(address)));
        }
        assert!(is_public_ip(IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))));
        assert!(!is_public_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(is_public_ip(IpAddr::V6(
            "2606:2800:220:1:248:1893:25c8:1946"
                .parse()
                .expect("test IPv6 address")
        )));
    }

    #[tokio::test]
    async fn fetch_pins_public_dns_and_revalidates_redirects() {
        let resolver = FakeResolver {
            answers: HashMap::from([
                (
                    "images.example.test".to_owned(),
                    vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))],
                ),
                (
                    "cdn.example.test".to_owned(),
                    vec![IpAddr::V4(Ipv4Addr::new(203, 0, 114, 8))],
                ),
            ]),
        };
        let transport = FakeTransport {
            responses: Mutex::new(VecDeque::from([
                response(
                    StatusCode::FOUND,
                    Some("https://cdn.example.test/final.png"),
                    None,
                    &[],
                ),
                response(
                    StatusCode::OK,
                    None,
                    Some("image/png; charset=binary"),
                    ONE_PIXEL_PNG,
                ),
            ])),
            calls: Mutex::new(Vec::new()),
        };
        let result = fetch_with(
            &Url::parse("https://images.example.test/start.png").expect("test URL"),
            &resolver,
            &transport,
        )
        .await
        .expect("public redirect should load");

        assert_eq!(result.media_type, "image/png");
        assert!(result.data_url.starts_with("data:image/png;base64,"));
        let calls = transport.calls.lock().expect("calls lock");
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].1.ip(), IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)));
        assert_eq!(calls[1].1.ip(), IpAddr::V4(Ipv4Addr::new(203, 0, 114, 8)));
    }

    #[tokio::test]
    async fn private_dns_and_redirect_targets_never_reach_the_transport() {
        let private_resolver = FakeResolver {
            answers: HashMap::from([(
                "images.example.test".to_owned(),
                vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
            )]),
        };
        let transport = FakeTransport {
            responses: Mutex::new(VecDeque::new()),
            calls: Mutex::new(Vec::new()),
        };
        let error = fetch_with(
            &Url::parse("https://images.example.test/start.png").expect("test URL"),
            &private_resolver,
            &transport,
        )
        .await
        .expect_err("private DNS should be rejected");
        assert_eq!(error, RemoteImageError::Invalid);
        assert!(transport.calls.lock().expect("calls lock").is_empty());

        let public_resolver = FakeResolver {
            answers: HashMap::from([(
                "images.example.test".to_owned(),
                vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))],
            )]),
        };
        let redirect_transport = FakeTransport {
            responses: Mutex::new(VecDeque::from([response(
                StatusCode::FOUND,
                Some("https://127.0.0.1/private.png"),
                None,
                &[],
            )])),
            calls: Mutex::new(Vec::new()),
        };
        let error = fetch_with(
            &Url::parse("https://images.example.test/start.png").expect("test URL"),
            &public_resolver,
            &redirect_transport,
        )
        .await
        .expect_err("private redirect should be rejected");
        assert_eq!(error, RemoteImageError::Invalid);
        assert_eq!(
            redirect_transport.calls.lock().expect("calls lock").len(),
            1
        );
    }

    #[test]
    fn response_requires_matching_safe_type_size_and_dimensions() {
        assert!(validate_image_response(Some("image/png"), ONE_PIXEL_PNG).is_ok());
        assert_eq!(
            validate_image_response(Some("image/jpeg"), ONE_PIXEL_PNG),
            Err(RemoteImageError::Invalid)
        );
        assert_eq!(
            validate_image_response(Some("image/svg+xml"), b"<svg></svg>"),
            Err(RemoteImageError::Invalid)
        );
        assert_eq!(
            validate_image_response(Some("image/png"), &vec![0_u8; MAX_REMOTE_IMAGE_BYTES + 1]),
            Err(RemoteImageError::Invalid)
        );
        let mut oversized = ONE_PIXEL_PNG.to_vec();
        oversized[16..20].copy_from_slice(&9_000_u32.to_be_bytes());
        assert_eq!(
            validate_image_response(Some("image/png"), &oversized),
            Err(RemoteImageError::Invalid)
        );
    }

    #[test]
    fn request_has_no_credentials_cookie_or_referrer_headers() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder().build().expect("test client");
        let request = build_request(
            &client,
            &Url::parse("https://images.example.test/a.png").expect("test URL"),
        )
        .build()
        .expect("test request");
        assert!(request.headers().get(header::AUTHORIZATION).is_none());
        assert!(request.headers().get(header::COOKIE).is_none());
        assert!(request.headers().get(header::REFERER).is_none());
        assert_eq!(
            request
                .headers()
                .get(header::ACCEPT)
                .and_then(|v| v.to_str().ok()),
            Some("image/png,image/jpeg,image/gif,image/webp;q=0.9,*/*;q=0.1")
        );
    }
}
