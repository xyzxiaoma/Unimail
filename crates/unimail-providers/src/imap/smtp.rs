use base64::{Engine as _, engine::general_purpose::STANDARD};
use rustls::RootCertStore;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use unimail_core::{
    AcceptedSend, Cancellation, ComposedMessage, ProviderError, ProviderErrorKind, ProviderResult,
    ReconciliationKey, RejectedSend, RetryHint, SendOutcome, SensitiveString, UnknownSend,
};

use super::{
    preset::ImapSmtpPreset,
    tls::{connect_verified_tls, platform_roots},
};

const MAX_RESPONSE_BYTES: usize = 16 * 1024;
const MAX_RESPONSE_LINES: usize = 64;

#[derive(Clone, Copy)]
struct SmtpResponse {
    code: u16,
}

pub(super) async fn submit(
    preset: &ImapSmtpPreset,
    account_address: &str,
    authorization_code: &SensitiveString,
    message: &ComposedMessage,
    cancellation: &dyn Cancellation,
) -> ProviderResult<SendOutcome> {
    submit_at(
        preset.smtp_host,
        preset.smtp_port,
        platform_roots(),
        account_address,
        authorization_code,
        message,
        cancellation,
    )
    .await
}

async fn submit_at(
    host: &str,
    port: u16,
    roots: RootCertStore,
    account_address: &str,
    authorization_code: &SensitiveString,
    message: &ComposedMessage,
    cancellation: &dyn Cancellation,
) -> ProviderResult<SendOutcome> {
    if cancellation.is_cancelled() {
        return Err(cancelled_error());
    }
    validate_submission(account_address, authorization_code, message)?;
    let stream = tokio::select! {
        () = cancellation.cancelled() => return Err(cancelled_error()),
        result = connect_verified_tls(host, port, roots) => result.map_err(map_tls_error)?,
    };
    let mut stream = BufReader::new(stream);
    expect_code(&mut stream, 220, cancellation).await?;
    command(&mut stream, b"EHLO unimail.invalid\r\n", cancellation).await?;
    expect_code(&mut stream, 250, cancellation).await?;
    command(&mut stream, b"AUTH LOGIN\r\n", cancellation).await?;
    expect_code(&mut stream, 334, cancellation)
        .await
        .map_err(map_auth_error)?;
    let username = format!("{}\r\n", STANDARD.encode(account_address.as_bytes()));
    command(&mut stream, username.as_bytes(), cancellation).await?;
    expect_code(&mut stream, 334, cancellation)
        .await
        .map_err(map_auth_error)?;
    let password = format!(
        "{}\r\n",
        STANDARD.encode(authorization_code.expose().as_bytes())
    );
    command(&mut stream, password.as_bytes(), cancellation).await?;
    expect_code(&mut stream, 235, cancellation)
        .await
        .map_err(map_auth_error)?;

    let mail_from = format!("MAIL FROM:<{}>\r\n", message.envelope.from);
    command(&mut stream, mail_from.as_bytes(), cancellation).await?;
    let response = read_response(&mut stream, cancellation).await?;
    classify_pre_data(response, "smtp_sender_rejected")?;

    for recipient in &message.envelope.recipients {
        let command_line = format!("RCPT TO:<{recipient}>\r\n");
        command(&mut stream, command_line.as_bytes(), cancellation).await?;
        let response = read_response(&mut stream, cancellation).await?;
        if response.code >= 500 {
            return Ok(SendOutcome::Rejected(RejectedSend {
                code: "smtp_recipient_rejected",
            }));
        }
        classify_pre_data(response, "smtp_recipient_rejected")?;
    }

    command(&mut stream, b"DATA\r\n", cancellation).await?;
    let response = read_response(&mut stream, cancellation).await?;
    if response.code >= 500 {
        return Ok(SendOutcome::Rejected(RejectedSend {
            code: "smtp_message_rejected",
        }));
    }
    classify_pre_data(response, "smtp_message_rejected")?;

    let wire_message = dot_stuffed(message.as_bytes());
    if command(&mut stream, &wire_message, cancellation)
        .await
        .is_err()
        || command(&mut stream, b".\r\n", cancellation).await.is_err()
    {
        return Ok(unknown(message));
    }
    let final_response = match read_response(&mut stream, cancellation).await {
        Ok(response) => response,
        Err(error) if error.kind == ProviderErrorKind::Cancelled => return Err(error),
        Err(_) => return Ok(unknown(message)),
    };
    match final_response.code {
        200..=299 => Ok(SendOutcome::Accepted(AcceptedSend {
            provider_message_id: None,
            reconciliation_key: ReconciliationKey::new(message.message_id.clone()),
        })),
        400..=499 => Err(transient_error("smtp_submission_deferred")),
        _ => Ok(SendOutcome::Rejected(RejectedSend {
            code: "smtp_message_rejected",
        })),
    }
}

fn validate_submission(
    account_address: &str,
    authorization_code: &SensitiveString,
    message: &ComposedMessage,
) -> ProviderResult<()> {
    if account_address.is_empty()
        || authorization_code.expose().is_empty()
        || message.envelope.from.is_empty()
        || message.envelope.recipients.is_empty()
        || message.message_id.is_empty()
        || message.as_bytes().is_empty()
    {
        return Err(ProviderError::new(
            ProviderErrorKind::Permanent,
            "smtp_submission_invalid",
        ));
    }
    Ok(())
}

async fn command<S>(
    stream: &mut BufReader<S>,
    bytes: &[u8],
    cancellation: &dyn Cancellation,
) -> ProviderResult<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    tokio::select! {
        () = cancellation.cancelled() => Err(cancelled_error()),
        result = stream.get_mut().write_all(bytes) => {
            result.map_err(|_| transient_error("smtp_connection_lost"))
        },
    }
}

async fn expect_code<S>(
    stream: &mut BufReader<S>,
    expected: u16,
    cancellation: &dyn Cancellation,
) -> ProviderResult<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let response = read_response(stream, cancellation).await?;
    if response.code == expected {
        Ok(())
    } else if response.code >= 500 {
        Err(ProviderError::new(
            ProviderErrorKind::Permanent,
            "smtp_command_rejected",
        ))
    } else {
        Err(transient_error("smtp_unexpected_response"))
    }
}

async fn read_response<S>(
    stream: &mut BufReader<S>,
    cancellation: &dyn Cancellation,
) -> ProviderResult<SmtpResponse>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut total = 0_usize;
    let mut expected_code = None;
    for _ in 0..MAX_RESPONSE_LINES {
        let mut line = String::new();
        let count = tokio::select! {
            () = cancellation.cancelled() => return Err(cancelled_error()),
            result = stream.read_line(&mut line) => {
                result.map_err(|_| transient_error("smtp_connection_lost"))?
            },
        };
        if count == 0 {
            return Err(transient_error("smtp_connection_lost"));
        }
        total = total.saturating_add(count);
        if total > MAX_RESPONSE_BYTES || line.len() < 4 {
            return Err(protocol_error("smtp_response_invalid"));
        }
        let code = line[..3]
            .parse::<u16>()
            .map_err(|_| protocol_error("smtp_response_invalid"))?;
        if expected_code
            .replace(code)
            .is_some_and(|expected| expected != code)
        {
            return Err(protocol_error("smtp_response_invalid"));
        }
        match line.as_bytes()[3] {
            b' ' => return Ok(SmtpResponse { code }),
            b'-' => {}
            _ => return Err(protocol_error("smtp_response_invalid")),
        }
    }
    Err(protocol_error("smtp_response_too_large"))
}

fn classify_pre_data(response: SmtpResponse, rejected_code: &'static str) -> ProviderResult<()> {
    match response.code {
        200..=399 => Ok(()),
        400..=499 => Err(transient_error("smtp_temporarily_unavailable")),
        _ => Err(ProviderError::new(
            ProviderErrorKind::Permanent,
            rejected_code,
        )),
    }
}

fn dot_stuffed(bytes: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(bytes.len().saturating_add(2));
    let mut line_start = true;
    for &byte in bytes {
        if line_start && byte == b'.' {
            output.push(b'.');
        }
        output.push(byte);
        line_start = byte == b'\n';
    }
    if !output.ends_with(b"\r\n") {
        output.extend_from_slice(b"\r\n");
    }
    output
}

fn unknown(message: &ComposedMessage) -> SendOutcome {
    SendOutcome::UnknownAfterSubmission(UnknownSend {
        reconciliation_key: ReconciliationKey::new(message.message_id.clone()),
    })
}

fn map_tls_error(error: ProviderError) -> ProviderError {
    if error.kind == ProviderErrorKind::Cancelled {
        error
    } else {
        transient_error("smtp_tls_connection_failed")
    }
}

fn map_auth_error(error: ProviderError) -> ProviderError {
    if error.kind == ProviderErrorKind::Cancelled {
        error
    } else {
        ProviderError::new(
            ProviderErrorKind::Authentication,
            "smtp_authentication_rejected",
        )
    }
}

fn transient_error(code: &'static str) -> ProviderError {
    ProviderError::new(ProviderErrorKind::Transient, code).with_retry(RetryHint::Backoff)
}

fn protocol_error(code: &'static str) -> ProviderError {
    ProviderError::new(ProviderErrorKind::Protocol, code)
}

fn cancelled_error() -> ProviderError {
    ProviderError::new(ProviderErrorKind::Cancelled, "smtp_cancelled")
}

#[cfg(test)]
mod tests {
    use unimail_core::{CancellationFuture, DeliveryEnvelope, ProviderErrorKind, SendOutcome};

    use super::*;
    use crate::imap::test_support::{ScriptStep, ScriptedTlsServer, TestCertificate};

    struct NeverCancelled;

    impl Cancellation for NeverCancelled {
        fn is_cancelled(&self) -> bool {
            false
        }

        fn cancelled(&self) -> CancellationFuture<'_> {
            Box::pin(std::future::pending())
        }
    }

    fn message() -> ComposedMessage {
        ComposedMessage::new(
            b"From: owner@example.test\r\nTo: recipient@example.test\r\nMessage-ID: <send@example.test>\r\n\r\n.leading dot\r\nbody\r\n".to_vec(),
            "<send@example.test>".to_owned(),
            DeliveryEnvelope {
                from: "owner@example.test".to_owned(),
                recipients: vec!["recipient@example.test".to_owned()],
            },
        )
    }

    fn authenticated_script(after_rcpt: Vec<ScriptStep>) -> Vec<ScriptStep> {
        let mut steps = vec![
            ScriptStep::Send(vec![b"220 smtp.example ESMTP\r\n".to_vec()]),
            ScriptStep::ExpectContains(b"EHLO unimail.invalid\r\n".to_vec()),
            ScriptStep::Send(vec![
                b"250-smtp.example\r\n".to_vec(),
                b"250 AUTH LOGIN\r\n".to_vec(),
            ]),
            ScriptStep::ExpectContains(b"AUTH LOGIN\r\n".to_vec()),
            ScriptStep::Send(vec![b"334 VXNlcm5hbWU6\r\n".to_vec()]),
            ScriptStep::ExpectContains(
                format!("{}\r\n", STANDARD.encode(b"owner@example.test")).into_bytes(),
            ),
            ScriptStep::Send(vec![b"334 UGFzc3dvcmQ6\r\n".to_vec()]),
            ScriptStep::ExpectContains(
                format!("{}\r\n", STANDARD.encode(b"fictional-code")).into_bytes(),
            ),
            ScriptStep::Send(vec![b"235 2.7.0 authenticated\r\n".to_vec()]),
            ScriptStep::ExpectContains(b"MAIL FROM:<owner@example.test>\r\n".to_vec()),
            ScriptStep::Send(vec![b"250 2.1.0 sender accepted\r\n".to_vec()]),
            ScriptStep::ExpectContains(b"RCPT TO:<recipient@example.test>\r\n".to_vec()),
        ];
        steps.extend(after_rcpt);
        steps
    }

    #[tokio::test]
    async fn accepted_submission_preserves_message_id_and_dot_stuffs() {
        let certificate = TestCertificate::localhost();
        let server = ScriptedTlsServer::spawn_script(
            &certificate,
            authenticated_script(vec![
                ScriptStep::Send(vec![b"250 2.1.5 recipient accepted\r\n".to_vec()]),
                ScriptStep::ExpectContains(b"DATA\r\n".to_vec()),
                ScriptStep::Send(vec![b"354 send message\r\n".to_vec()]),
                ScriptStep::ExpectContains(b"\r\n.\r\n".to_vec()),
                ScriptStep::Send(vec![b"250 2.0.0 queued\r\n".to_vec()]),
            ]),
        )
        .await;
        let outcome = submit_at(
            "localhost",
            server.port(),
            certificate.roots(),
            "owner@example.test",
            &SensitiveString::new("fictional-code"),
            &message(),
            &NeverCancelled,
        )
        .await
        .unwrap();
        let SendOutcome::Accepted(accepted) = outcome else {
            panic!("expected accepted submission")
        };
        assert_eq!(accepted.reconciliation_key.expose(), "<send@example.test>");
        assert!(
            server
                .transcript()
                .last()
                .unwrap()
                .windows(b"\r\n..leading dot\r\n".len())
                .any(|window| window == b"\r\n..leading dot\r\n")
        );
        server.finish().await;
    }

    #[tokio::test]
    async fn recipient_rejection_is_terminal_and_safe() {
        let certificate = TestCertificate::localhost();
        let server = ScriptedTlsServer::spawn_script(
            &certificate,
            authenticated_script(vec![ScriptStep::Send(vec![
                b"550 5.1.1 private recipient detail\r\n".to_vec(),
            ])]),
        )
        .await;
        let outcome = submit_at(
            "localhost",
            server.port(),
            certificate.roots(),
            "owner@example.test",
            &SensitiveString::new("fictional-code"),
            &message(),
            &NeverCancelled,
        )
        .await
        .unwrap();
        assert_eq!(
            outcome,
            SendOutcome::Rejected(RejectedSend {
                code: "smtp_recipient_rejected"
            })
        );
        assert!(!format!("{outcome:?}").contains("private recipient detail"));
        server.finish().await;
    }

    #[tokio::test]
    async fn disconnect_after_data_is_unknown_and_never_transient_retry() {
        let certificate = TestCertificate::localhost();
        let server = ScriptedTlsServer::spawn_script(
            &certificate,
            authenticated_script(vec![
                ScriptStep::Send(vec![b"250 2.1.5 recipient accepted\r\n".to_vec()]),
                ScriptStep::ExpectContains(b"DATA\r\n".to_vec()),
                ScriptStep::Send(vec![b"354 send message\r\n".to_vec()]),
                ScriptStep::ExpectContains(b"\r\n.\r\n".to_vec()),
                ScriptStep::Disconnect,
            ]),
        )
        .await;
        let outcome = submit_at(
            "localhost",
            server.port(),
            certificate.roots(),
            "owner@example.test",
            &SensitiveString::new("fictional-code"),
            &message(),
            &NeverCancelled,
        )
        .await
        .unwrap();
        assert!(matches!(outcome, SendOutcome::UnknownAfterSubmission(_)));
        server.finish().await;
    }

    #[test]
    fn authentication_mapping_never_exposes_server_text() {
        let error = map_auth_error(ProviderError::new(
            ProviderErrorKind::Permanent,
            "private_server_text",
        ));
        assert_eq!(error.kind, ProviderErrorKind::Authentication);
        assert_eq!(error.code, "smtp_authentication_rejected");
    }
}
