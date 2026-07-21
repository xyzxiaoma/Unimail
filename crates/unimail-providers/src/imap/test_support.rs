use std::sync::{Arc, Mutex};

use rcgen::generate_simple_self_signed;
use rustls::{
    RootCertStore, ServerConfig,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};
use tokio_rustls::TlsAcceptor;

pub(super) struct TestCertificate {
    certificate: CertificateDer<'static>,
    private_key: Vec<u8>,
}

impl TestCertificate {
    pub(super) fn localhost() -> Self {
        let generated = generate_simple_self_signed(["localhost".to_owned()]).unwrap();
        Self {
            certificate: generated.cert.der().clone(),
            private_key: generated.signing_key.serialize_der(),
        }
    }

    pub(super) fn roots(&self) -> RootCertStore {
        let mut roots = RootCertStore::empty();
        roots.add(self.certificate.clone()).unwrap();
        roots
    }

    fn server_config(&self) -> ServerConfig {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(self.private_key.clone()));
        ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![self.certificate.clone()], key)
            .unwrap()
    }
}

pub(super) struct ScriptedTlsServer {
    port: u16,
    task: JoinHandle<()>,
    transcript: Arc<Mutex<Vec<Vec<u8>>>>,
}

pub(super) enum ScriptStep {
    Send(Vec<Vec<u8>>),
    ExpectContains(Vec<u8>),
    Disconnect,
}

impl ScriptedTlsServer {
    pub(super) async fn spawn(certificate: &TestCertificate, fragments: Vec<Vec<u8>>) -> Self {
        Self::spawn_script(certificate, vec![ScriptStep::Send(fragments)]).await
    }

    pub(super) async fn spawn_script(
        certificate: &TestCertificate,
        steps: Vec<ScriptStep>,
    ) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let acceptor = TlsAcceptor::from(Arc::new(certificate.server_config()));
        let transcript = Arc::new(Mutex::new(Vec::new()));
        let task_transcript = transcript.clone();
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let Ok(mut stream) = acceptor.accept(stream).await else {
                return;
            };
            for step in steps {
                match step {
                    ScriptStep::Send(fragments) => {
                        for fragment in fragments {
                            stream.write_all(&fragment).await.unwrap();
                            tokio::task::yield_now().await;
                        }
                    }
                    ScriptStep::ExpectContains(expected) => {
                        let mut received = Vec::new();
                        let mut buffer = [0_u8; 256];
                        while !received
                            .windows(expected.len())
                            .any(|window| window == expected)
                        {
                            let count = stream.read(&mut buffer).await.unwrap();
                            assert_ne!(
                                count, 0,
                                "client disconnected before scripted input arrived"
                            );
                            received.extend_from_slice(&buffer[..count]);
                        }
                        task_transcript.lock().unwrap().push(received);
                    }
                    ScriptStep::Disconnect => return,
                }
            }
            stream.shutdown().await.unwrap();
        });
        Self {
            port,
            task,
            transcript,
        }
    }

    pub(super) const fn port(&self) -> u16 {
        self.port
    }

    pub(super) async fn finish(self) {
        self.task.await.unwrap();
    }

    pub(super) fn transcript(&self) -> Vec<Vec<u8>> {
        self.transcript.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::imap::tls::connect_verified_tls;

    #[tokio::test]
    async fn script_supports_capabilities_and_records_client_commands() {
        let certificate = TestCertificate::localhost();
        let server = ScriptedTlsServer::spawn_script(
            &certificate,
            vec![
                ScriptStep::Send(vec![b"* OK ready\r\n".to_vec()]),
                ScriptStep::ExpectContains(b"CAPABILITY\r\n".to_vec()),
                ScriptStep::Send(vec![
                    b"* CAPABILITY IMAP4rev1 ".to_vec(),
                    b"CONDSTORE ID\r\na1 OK done\r\n".to_vec(),
                ]),
            ],
        )
        .await;
        let mut client = connect_verified_tls("localhost", server.port(), certificate.roots())
            .await
            .unwrap();
        let mut greeting = [0_u8; 12];
        client.read_exact(&mut greeting).await.unwrap();
        client.write_all(b"a1 CAPABILITY\r\n").await.unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert!(response.windows(9).any(|window| window == b"CONDSTORE"));
        assert!(
            server.transcript()[0]
                .windows(10)
                .any(|window| window == b"CAPABILITY")
        );
        server.finish().await;
    }

    #[tokio::test]
    async fn script_can_disconnect_after_smtp_data() {
        let certificate = TestCertificate::localhost();
        let server = ScriptedTlsServer::spawn_script(
            &certificate,
            vec![
                ScriptStep::Send(vec![b"220 smtp.example ESMTP\r\n".to_vec()]),
                ScriptStep::ExpectContains(b"\r\n.\r\n".to_vec()),
                ScriptStep::Disconnect,
            ],
        )
        .await;
        let mut client = connect_verified_tls("localhost", server.port(), certificate.roots())
            .await
            .unwrap();
        let mut greeting = [0_u8; 24];
        client.read_exact(&mut greeting).await.unwrap();
        client
            .write_all(b"DATA\r\nFrom: sender@example.test\r\n\r\nbody\r\n.\r\n")
            .await
            .unwrap();
        let mut response = [0_u8; 1];
        let error = client.read(&mut response).await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);
        assert!(
            server.transcript()[0]
                .windows(5)
                .any(|window| window == b"\r\n.\r\n")
        );
        server.finish().await;
    }
}
