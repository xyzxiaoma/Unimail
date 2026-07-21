use std::sync::Arc;

use rustls::{ClientConfig, RootCertStore, pki_types::ServerName};
use tokio::net::TcpStream;
use tokio_rustls::{TlsConnector, client::TlsStream};
use unimail_core::{ProviderError, ProviderErrorKind, ProviderResult, RetryHint};

pub(super) fn platform_roots() -> RootCertStore {
    webpki_roots::TLS_SERVER_ROOTS.iter().cloned().collect()
}

pub(super) async fn connect_verified_tls(
    host: &str,
    port: u16,
    roots: RootCertStore,
) -> ProviderResult<TlsStream<TcpStream>> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let server_name = ServerName::try_from(host.to_owned()).map_err(|_| {
        ProviderError::new(ProviderErrorKind::Permanent, "imap_tls_server_name_invalid")
    })?;
    let tcp = TcpStream::connect((host, port)).await.map_err(|_| {
        ProviderError::new(ProviderErrorKind::Transient, "imap_connect_failed")
            .with_retry(RetryHint::Backoff)
    })?;
    let config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    TlsConnector::from(Arc::new(config))
        .connect(server_name, tcp)
        .await
        .map_err(|_| {
            ProviderError::new(ProviderErrorKind::Transient, "imap_tls_handshake_failed")
                .with_retry(RetryHint::Backoff)
        })
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::imap::test_support::{ScriptedTlsServer, TestCertificate};

    #[tokio::test]
    async fn trusted_test_ca_accepts_fragmented_tls_frames() {
        let certificate = TestCertificate::localhost();
        let server = ScriptedTlsServer::spawn(
            &certificate,
            vec![b"* OK ".to_vec(), b"IMAP ready\r\n".to_vec()],
        )
        .await;
        let mut stream = connect_verified_tls("localhost", server.port(), certificate.roots())
            .await
            .unwrap();
        let mut greeting = Vec::new();
        stream.read_to_end(&mut greeting).await.unwrap();
        assert_eq!(greeting, b"* OK IMAP ready\r\n");
        server.finish().await;
    }

    #[tokio::test]
    async fn untrusted_certificate_is_rejected() {
        let certificate = TestCertificate::localhost();
        let server = ScriptedTlsServer::spawn(&certificate, vec![b"* OK ready\r\n".to_vec()]).await;
        let error = connect_verified_tls("localhost", server.port(), platform_roots())
            .await
            .unwrap_err();
        assert_eq!(error.code, "imap_tls_handshake_failed");
        server.finish().await;
    }

    #[tokio::test]
    async fn plaintext_server_cannot_trigger_a_downgrade() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            stream
                .write_all(b"* OK plaintext is forbidden\r\n")
                .await
                .unwrap();
        });
        let certificate = TestCertificate::localhost();
        let error = connect_verified_tls("localhost", port, certificate.roots())
            .await
            .unwrap_err();
        assert_eq!(error.code, "imap_tls_handshake_failed");
        task.await.unwrap();
    }
}
