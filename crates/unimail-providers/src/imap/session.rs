use std::collections::HashSet;

use async_imap::{Session, error::Error as ImapError};
use futures_util::TryStreamExt;
use rustls::RootCertStore;
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use unimail_core::{
    Cancellation, ProviderError, ProviderErrorKind, ProviderResult, RetryHint, SensitiveString,
};

use super::{
    preset::ImapSmtpPreset,
    tls::{connect_verified_tls, platform_roots},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum ImapCapability {
    Condstore,
    Qresync,
    SpecialUse,
    Id,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ImapCapabilities {
    values: HashSet<ImapCapability>,
}

impl ImapCapabilities {
    pub(super) fn has(&self, capability: ImapCapability) -> bool {
        self.values.contains(&capability)
    }
}

pub(super) struct ImapSession {
    inner: Session<TlsStream<TcpStream>>,
    capabilities: ImapCapabilities,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct DiscoveredMailboxes {
    pub inbox: String,
    pub sent: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SelectedMailbox {
    pub uid_validity: u32,
    pub uid_next: Option<u32>,
    pub highest_modseq: Option<u64>,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct FetchedMessage {
    pub uid: u32,
    pub modseq: Option<u64>,
    pub seen: bool,
    pub received_at_ms: i64,
    pub raw: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FetchedFlags {
    pub uid: u32,
    pub modseq: Option<u64>,
    pub seen: bool,
}

impl std::fmt::Debug for FetchedMessage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FetchedMessage")
            .field("has_uid", &(self.uid != 0))
            .field("has_modseq", &self.modseq.is_some())
            .field("seen", &self.seen)
            .field("has_received_at", &(self.received_at_ms != 0))
            .field("raw_bytes", &self.raw.len())
            .finish()
    }
}

impl std::fmt::Debug for DiscoveredMailboxes {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DiscoveredMailboxes")
            .field("has_inbox", &!self.inbox.is_empty())
            .field("has_sent", &self.sent.is_some())
            .finish()
    }
}

impl ImapSession {
    pub(super) fn capabilities(&self) -> &ImapCapabilities {
        &self.capabilities
    }

    pub(super) async fn discover_mailboxes(
        &mut self,
        preset: &ImapSmtpPreset,
        cancellation: &dyn Cancellation,
    ) -> ProviderResult<DiscoveredMailboxes> {
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        let mut names = tokio::select! {
            () = cancellation.cancelled() => return Err(cancelled_error()),
            result = self.inner.list(Some(""), Some("*")) => {
                result.map_err(|error| map_session_error(&error))?
            },
        };
        let mut inbox = None;
        let mut special_use_sent = None;
        let mut fallback_sent = None;
        loop {
            let name = tokio::select! {
                () = cancellation.cancelled() => return Err(cancelled_error()),
                result = names.try_next() => result.map_err(|error| map_session_error(&error))?,
            };
            let Some(name) = name else {
                break;
            };
            if name
                .attributes()
                .iter()
                .any(|attribute| matches!(attribute, async_imap::types::NameAttribute::NoSelect))
            {
                continue;
            }
            let mailbox_name = name.name();
            if mailbox_name.eq_ignore_ascii_case("INBOX") {
                inbox = Some(mailbox_name.to_owned());
            }
            if name
                .attributes()
                .iter()
                .any(|attribute| matches!(attribute, async_imap::types::NameAttribute::Sent))
            {
                special_use_sent = Some(mailbox_name.to_owned());
            } else if preset
                .sent_fallbacks
                .iter()
                .any(|fallback| mailbox_name.eq_ignore_ascii_case(fallback))
            {
                fallback_sent = Some(mailbox_name.to_owned());
            }
        }
        let inbox = inbox
            .ok_or_else(|| ProviderError::new(ProviderErrorKind::Protocol, "imap_inbox_missing"))?;
        Ok(DiscoveredMailboxes {
            inbox,
            sent: special_use_sent.or(fallback_sent),
        })
    }

    pub(super) async fn select_mailbox(
        &mut self,
        mailbox_id: &str,
        cancellation: &dyn Cancellation,
    ) -> ProviderResult<SelectedMailbox> {
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        let mailbox = if self.capabilities.has(ImapCapability::Condstore) {
            tokio::select! {
                () = cancellation.cancelled() => return Err(cancelled_error()),
                result = self.inner.select_condstore(mailbox_id) => {
                    result.map_err(|error| map_session_error(&error))?
                },
            }
        } else {
            tokio::select! {
                () = cancellation.cancelled() => return Err(cancelled_error()),
                result = self.inner.select(mailbox_id) => {
                    result.map_err(|error| map_session_error(&error))?
                },
            }
        };
        let uid_validity = mailbox.uid_validity.ok_or_else(|| {
            ProviderError::new(ProviderErrorKind::Protocol, "imap_uidvalidity_missing")
        })?;
        if uid_validity == 0 {
            return Err(ProviderError::new(
                ProviderErrorKind::Protocol,
                "imap_uidvalidity_invalid",
            ));
        }
        Ok(SelectedMailbox {
            uid_validity,
            uid_next: mailbox.uid_next,
            highest_modseq: mailbox.highest_modseq,
        })
    }

    pub(super) async fn search_all_uids(
        &mut self,
        cancellation: &dyn Cancellation,
    ) -> ProviderResult<Vec<u32>> {
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        let uids = tokio::select! {
            () = cancellation.cancelled() => return Err(cancelled_error()),
            result = self.inner.uid_search("ALL") => {
                result.map_err(|error| map_session_error(&error))?
            },
        };
        Ok(uids.into_iter().collect())
    }

    pub(super) async fn search_message_id(
        &mut self,
        message_id: &str,
        cancellation: &dyn Cancellation,
    ) -> ProviderResult<Vec<u32>> {
        if message_id.is_empty() || message_id.len() > 998 || message_id.contains(['\r', '\n']) {
            return Err(ProviderError::new(
                ProviderErrorKind::Permanent,
                "imap_message_id_invalid",
            ));
        }
        let quoted = message_id.replace('\\', "\\\\").replace('"', "\\\"");
        let query = format!("HEADER Message-ID \"{quoted}\"");
        let uids = tokio::select! {
            () = cancellation.cancelled() => return Err(cancelled_error()),
            result = self.inner.uid_search(query) => {
                result.map_err(|error| map_session_error(&error))?
            },
        };
        let mut uids: Vec<u32> = uids.into_iter().collect();
        uids.sort_unstable();
        Ok(uids)
    }

    pub(super) async fn fetch_uids(
        &mut self,
        uid_set: &str,
        cancellation: &dyn Cancellation,
    ) -> ProviderResult<Vec<FetchedMessage>> {
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        let mut fetches = tokio::select! {
            () = cancellation.cancelled() => return Err(cancelled_error()),
            result = self.inner.uid_fetch(uid_set, "(UID FLAGS MODSEQ INTERNALDATE BODY.PEEK[])") => {
                result.map_err(|error| map_session_error(&error))?
            },
        };
        let mut messages = Vec::new();
        loop {
            let fetch = tokio::select! {
                () = cancellation.cancelled() => return Err(cancelled_error()),
                result = fetches.try_next() => result.map_err(|error| map_session_error(&error))?,
            };
            let Some(fetch) = fetch else {
                break;
            };
            let uid = fetch.uid.ok_or_else(|| {
                ProviderError::new(ProviderErrorKind::Protocol, "imap_fetch_uid_missing")
            })?;
            let raw = fetch.body().ok_or_else(|| {
                ProviderError::new(ProviderErrorKind::Protocol, "imap_fetch_body_missing")
            })?;
            let received_at_ms = fetch
                .internal_date()
                .ok_or_else(|| {
                    ProviderError::new(ProviderErrorKind::Protocol, "imap_internal_date_missing")
                })?
                .timestamp_millis();
            messages.push(FetchedMessage {
                uid,
                modseq: fetch.modseq,
                seen: fetch
                    .flags()
                    .any(|flag| matches!(flag, async_imap::types::Flag::Seen)),
                received_at_ms,
                raw: raw.to_vec(),
            });
        }
        messages.sort_unstable_by_key(|message| message.uid);
        Ok(messages)
    }

    pub(super) async fn fetch_flags(
        &mut self,
        uid: u32,
        cancellation: &dyn Cancellation,
    ) -> ProviderResult<FetchedFlags> {
        let mut fetches = tokio::select! {
            () = cancellation.cancelled() => return Err(cancelled_error()),
            result = self.inner.uid_fetch(uid.to_string(), "(UID FLAGS MODSEQ)") => {
                result.map_err(|error| map_session_error(&error))?
            },
        };
        let fetch = tokio::select! {
            () = cancellation.cancelled() => return Err(cancelled_error()),
            result = fetches.try_next() => result.map_err(|error| map_session_error(&error))?,
        }
        .ok_or_else(|| ProviderError::new(ProviderErrorKind::Permanent, "imap_message_missing"))?;
        let fetched_uid = fetch.uid.ok_or_else(|| {
            ProviderError::new(ProviderErrorKind::Protocol, "imap_fetch_uid_missing")
        })?;
        if fetched_uid != uid {
            return Err(ProviderError::new(
                ProviderErrorKind::Protocol,
                "imap_fetch_uid_mismatch",
            ));
        }
        let flags = FetchedFlags {
            uid,
            modseq: fetch.modseq,
            seen: fetch
                .flags()
                .any(|flag| matches!(flag, async_imap::types::Flag::Seen)),
        };
        while tokio::select! {
            () = cancellation.cancelled() => return Err(cancelled_error()),
            result = fetches.try_next() => result.map_err(|error| map_session_error(&error))?,
        }
        .is_some()
        {}
        Ok(flags)
    }

    pub(super) async fn store_seen(
        &mut self,
        uid: u32,
        desired_read: bool,
        unchanged_since: Option<u64>,
        cancellation: &dyn Cancellation,
    ) -> ProviderResult<()> {
        let operation = if desired_read {
            "+FLAGS.SILENT (\\Seen)"
        } else {
            "-FLAGS.SILENT (\\Seen)"
        };
        let query = unchanged_since.map_or_else(
            || operation.to_owned(),
            |modseq| format!("(UNCHANGEDSINCE {modseq}) {operation}"),
        );
        let mut responses = tokio::select! {
            () = cancellation.cancelled() => return Err(cancelled_error()),
            result = self.inner.uid_store(uid.to_string(), query) => {
                result.map_err(|error| map_session_error(&error))?
            },
        };
        while tokio::select! {
            () = cancellation.cancelled() => return Err(cancelled_error()),
            result = responses.try_next() => result.map_err(|error| map_session_error(&error))?,
        }
        .is_some()
        {}
        Ok(())
    }
}

pub(super) async fn connect(
    preset: &'static ImapSmtpPreset,
    account_address: &str,
    authorization_code: &SensitiveString,
    cancellation: &dyn Cancellation,
) -> ProviderResult<ImapSession> {
    connect_at(
        preset,
        account_address,
        authorization_code,
        preset.imap_host,
        preset.imap_port,
        platform_roots(),
        cancellation,
    )
    .await
}

async fn connect_at(
    preset: &'static ImapSmtpPreset,
    account_address: &str,
    authorization_code: &SensitiveString,
    host: &str,
    port: u16,
    roots: RootCertStore,
    cancellation: &dyn Cancellation,
) -> ProviderResult<ImapSession> {
    if cancellation.is_cancelled() {
        return Err(cancelled_error());
    }
    let account_address = preset.normalize_account_address(account_address)?;
    if authorization_code.expose().trim().is_empty() {
        return Err(ProviderError::new(
            ProviderErrorKind::Authentication,
            "imap_authorization_code_missing",
        ));
    }
    let stream = tokio::select! {
        () = cancellation.cancelled() => return Err(cancelled_error()),
        result = connect_verified_tls(host, port, roots) => result?,
    };
    let client = async_imap::Client::new(stream);
    let mut session = tokio::select! {
        () = cancellation.cancelled() => return Err(cancelled_error()),
        result = client.login(account_address, authorization_code.expose()) => {
            result.map_err(|(error, _)| map_login_error(&error))?
        },
    };
    let capabilities = tokio::select! {
        () = cancellation.cancelled() => return Err(cancelled_error()),
        result = session.capabilities() => result.map_err(|error| map_session_error(&error))?,
    };
    let capabilities = ImapCapabilities {
        values: [
            ("CONDSTORE", ImapCapability::Condstore),
            ("QRESYNC", ImapCapability::Qresync),
            ("SPECIAL-USE", ImapCapability::SpecialUse),
            ("ID", ImapCapability::Id),
        ]
        .into_iter()
        .filter_map(|(wire_name, capability)| capabilities.has_str(wire_name).then_some(capability))
        .collect(),
    };
    if preset.sends_client_id && capabilities.has(ImapCapability::Id) {
        tokio::select! {
            () = cancellation.cancelled() => return Err(cancelled_error()),
            result = session.id([
                ("name", Some("Unimail")),
                ("version", Some(env!("CARGO_PKG_VERSION"))),
                ("vendor", Some("Unimail")),
            ]) => result.map_err(|error| map_session_error(&error))?,
        };
    }
    Ok(ImapSession {
        inner: session,
        capabilities,
    })
}

fn map_login_error(error: &ImapError) -> ProviderError {
    match error {
        ImapError::No(_) => ProviderError::new(
            ProviderErrorKind::Authentication,
            "imap_authentication_rejected",
        ),
        other => map_session_error(other),
    }
}

fn map_session_error(error: &ImapError) -> ProviderError {
    match error {
        ImapError::Io(_) | ImapError::ConnectionLost => {
            ProviderError::new(ProviderErrorKind::Transient, "imap_connection_lost")
                .with_retry(RetryHint::Backoff)
        }
        ImapError::No(_) | ImapError::Bad(_) | ImapError::Parse(_) | ImapError::Append => {
            ProviderError::new(ProviderErrorKind::Protocol, "imap_protocol_error")
        }
        ImapError::Validate(_) => {
            ProviderError::new(ProviderErrorKind::Permanent, "imap_command_invalid")
        }
        _ => ProviderError::new(ProviderErrorKind::Protocol, "imap_protocol_error"),
    }
}

fn cancelled_error() -> ProviderError {
    ProviderError::new(ProviderErrorKind::Cancelled, "imap_cancelled")
}

#[cfg(test)]
mod tests {
    use unimail_core::{CancellationFuture, ProviderErrorKind};

    use super::*;
    use crate::imap::{
        NETEASE_PRESET, QQ_PRESET,
        test_support::{ScriptStep, ScriptedTlsServer, TestCertificate},
    };

    struct NeverCancelled;

    impl Cancellation for NeverCancelled {
        fn is_cancelled(&self) -> bool {
            false
        }

        fn cancelled(&self) -> CancellationFuture<'_> {
            Box::pin(std::future::pending())
        }
    }

    #[tokio::test]
    async fn qq_logs_in_and_negotiates_capabilities_without_id() {
        let certificate = TestCertificate::localhost();
        let server = ScriptedTlsServer::spawn_script(
            &certificate,
            vec![
                ScriptStep::Send(vec![b"* OK ready\r\n".to_vec()]),
                ScriptStep::ExpectContains(b"LOGIN".to_vec()),
                ScriptStep::Send(vec![b"A0001 OK LOGIN completed\r\n".to_vec()]),
                ScriptStep::ExpectContains(b"CAPABILITY".to_vec()),
                ScriptStep::Send(vec![
                    b"* CAPABILITY IMAP4rev1 CONDSTORE SPECIAL-USE\r\nA0002 OK completed\r\n"
                        .to_vec(),
                ]),
            ],
        )
        .await;
        let session = connect_at(
            &QQ_PRESET,
            "owner@qq.com",
            &SensitiveString::new("fictional-code"),
            "localhost",
            server.port(),
            certificate.roots(),
            &NeverCancelled,
        )
        .await
        .unwrap();
        assert_eq!(
            session.capabilities().values,
            HashSet::from([ImapCapability::Condstore, ImapCapability::SpecialUse])
        );
        assert_eq!(server.transcript().len(), 2);
        drop(session);
        server.finish().await;
    }

    #[tokio::test]
    async fn netease_sends_bounded_non_secret_id_when_advertised() {
        let certificate = TestCertificate::localhost();
        let server = ScriptedTlsServer::spawn_script(
            &certificate,
            vec![
                ScriptStep::Send(vec![b"* OK ready\r\n".to_vec()]),
                ScriptStep::ExpectContains(b"LOGIN".to_vec()),
                ScriptStep::Send(vec![b"A0001 OK LOGIN completed\r\n".to_vec()]),
                ScriptStep::ExpectContains(b"CAPABILITY".to_vec()),
                ScriptStep::Send(vec![
                    b"* CAPABILITY IMAP4rev1 ID QRESYNC\r\nA0002 OK completed\r\n".to_vec(),
                ]),
                ScriptStep::ExpectContains(b"ID (".to_vec()),
                ScriptStep::Send(vec![b"* ID NIL\r\nA0003 OK ID completed\r\n".to_vec()]),
            ],
        )
        .await;
        let session = connect_at(
            &NETEASE_PRESET,
            "owner@163.com",
            &SensitiveString::new("fictional-code"),
            "localhost",
            server.port(),
            certificate.roots(),
            &NeverCancelled,
        )
        .await
        .unwrap();
        assert!(session.capabilities().has(ImapCapability::Id));
        let transcript = server.transcript();
        assert_eq!(transcript.len(), 3);
        assert!(
            !transcript[2]
                .windows(b"fictional-code".len())
                .any(|window| window == b"fictional-code")
        );
        drop(session);
        server.finish().await;
    }

    #[tokio::test]
    async fn login_rejection_maps_to_a_safe_authentication_error() {
        let certificate = TestCertificate::localhost();
        let server = ScriptedTlsServer::spawn_script(
            &certificate,
            vec![
                ScriptStep::Send(vec![b"* OK ready\r\n".to_vec()]),
                ScriptStep::ExpectContains(b"LOGIN".to_vec()),
                ScriptStep::Send(vec![b"A0001 NO private provider response\r\n".to_vec()]),
            ],
        )
        .await;
        let error = connect_at(
            &QQ_PRESET,
            "owner@qq.com",
            &SensitiveString::new("fictional-code"),
            "localhost",
            server.port(),
            certificate.roots(),
            &NeverCancelled,
        )
        .await
        .err()
        .unwrap();
        assert_eq!(error.kind, ProviderErrorKind::Authentication);
        assert_eq!(error.code, "imap_authentication_rejected");
        assert!(!format!("{error:?}").contains("private provider response"));
        server.finish().await;
    }

    #[tokio::test]
    async fn mailbox_discovery_prefers_special_use_sent_and_requires_inbox() {
        let certificate = TestCertificate::localhost();
        let server = ScriptedTlsServer::spawn_script(
            &certificate,
            vec![
                ScriptStep::Send(vec![b"* OK ready\r\n".to_vec()]),
                ScriptStep::ExpectContains(b"LOGIN".to_vec()),
                ScriptStep::Send(vec![b"A0001 OK LOGIN completed\r\n".to_vec()]),
                ScriptStep::ExpectContains(b"CAPABILITY".to_vec()),
                ScriptStep::Send(vec![
                    b"* CAPABILITY IMAP4rev1 SPECIAL-USE\r\nA0002 OK completed\r\n".to_vec(),
                ]),
                ScriptStep::ExpectContains(b"LIST".to_vec()),
                ScriptStep::Send(vec![
                    b"* LIST () \"/\" \"INBOX\"\r\n".to_vec(),
                    b"* LIST () \"/\" \"Sent Messages\"\r\n".to_vec(),
                    b"* LIST (\\Sent) \"/\" \"Provider Sent\"\r\nA0003 OK LIST completed\r\n"
                        .to_vec(),
                ]),
            ],
        )
        .await;
        let mut session = connect_at(
            &QQ_PRESET,
            "owner@qq.com",
            &SensitiveString::new("fictional-code"),
            "localhost",
            server.port(),
            certificate.roots(),
            &NeverCancelled,
        )
        .await
        .unwrap();
        let mailboxes = session
            .discover_mailboxes(&QQ_PRESET, &NeverCancelled)
            .await
            .unwrap();
        assert_eq!(mailboxes.inbox, "INBOX");
        assert_eq!(mailboxes.sent.as_deref(), Some("Provider Sent"));
        assert!(!format!("{mailboxes:?}").contains("Provider Sent"));
        drop(session);
        server.finish().await;
    }

    #[tokio::test]
    async fn uid_sync_uses_condstore_uid_search_and_body_peek() {
        let certificate = TestCertificate::localhost();
        let raw = b"From: sender@example.test\r\nTo: owner@example.test\r\n\r\nhello";
        let fetch_response = format!(
            "* 1 FETCH (UID 42 FLAGS (\\Seen) MODSEQ (9) INTERNALDATE \"21-Jul-2026 12:00:00 +0800\" BODY[] {{{}}}\r\n{} )\r\nA0005 OK FETCH completed\r\n",
            raw.len(),
            String::from_utf8_lossy(raw)
        );
        let server = ScriptedTlsServer::spawn_script(
            &certificate,
            vec![
                ScriptStep::Send(vec![b"* OK ready\r\n".to_vec()]),
                ScriptStep::ExpectContains(b"LOGIN".to_vec()),
                ScriptStep::Send(vec![b"A0001 OK LOGIN completed\r\n".to_vec()]),
                ScriptStep::ExpectContains(b"CAPABILITY".to_vec()),
                ScriptStep::Send(vec![b"* CAPABILITY IMAP4rev1 CONDSTORE\r\nA0002 OK completed\r\n".to_vec()]),
                ScriptStep::ExpectContains(b"SELECT".to_vec()),
                ScriptStep::Send(vec![b"* FLAGS (\\Seen)\r\n* 1 EXISTS\r\n* OK [UIDVALIDITY 77] valid\r\n* OK [UIDNEXT 43] next\r\n* OK [HIGHESTMODSEQ 9] modseq\r\nA0003 OK [READ-WRITE] SELECT completed\r\n".to_vec()]),
                ScriptStep::ExpectContains(b"UID SEARCH ALL".to_vec()),
                ScriptStep::Send(vec![b"* SEARCH 42\r\nA0004 OK SEARCH completed\r\n".to_vec()]),
                ScriptStep::ExpectContains(b"BODY.PEEK[]".to_vec()),
                ScriptStep::Send(vec![fetch_response.into_bytes()]),
            ],
        )
        .await;
        let mut session = connect_at(
            &QQ_PRESET,
            "owner@qq.com",
            &SensitiveString::new("fictional-code"),
            "localhost",
            server.port(),
            certificate.roots(),
            &NeverCancelled,
        )
        .await
        .unwrap();
        let selected = session
            .select_mailbox("INBOX", &NeverCancelled)
            .await
            .unwrap();
        assert_eq!(selected.uid_validity, 77);
        assert_eq!(selected.highest_modseq, Some(9));
        let uids = session.search_all_uids(&NeverCancelled).await.unwrap();
        assert_eq!(uids, vec![42]);
        let messages = session.fetch_uids("42", &NeverCancelled).await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].uid, 42);
        assert!(messages[0].seen);
        assert_eq!(messages[0].raw, raw);
        assert!(!format!("{:?}", messages[0]).contains("sender@example.test"));
        let transcript = server.transcript();
        assert!(
            transcript[2]
                .windows(b"(CONDSTORE)".len())
                .any(|window| window == b"(CONDSTORE)")
        );
        assert!(
            transcript[4]
                .windows(b"BODY.PEEK[]".len())
                .any(|window| window == b"BODY.PEEK[]")
        );
        drop(session);
        server.finish().await;
    }

    #[tokio::test]
    async fn read_state_uses_uid_store_with_condstore_guard() {
        let certificate = TestCertificate::localhost();
        let server = ScriptedTlsServer::spawn_script(
            &certificate,
            vec![
                ScriptStep::Send(vec![b"* OK ready\r\n".to_vec()]),
                ScriptStep::ExpectContains(b"LOGIN".to_vec()),
                ScriptStep::Send(vec![b"A0001 OK LOGIN completed\r\n".to_vec()]),
                ScriptStep::ExpectContains(b"CAPABILITY".to_vec()),
                ScriptStep::Send(vec![b"* CAPABILITY IMAP4rev1 CONDSTORE\r\nA0002 OK completed\r\n".to_vec()]),
                ScriptStep::ExpectContains(b"SELECT".to_vec()),
                ScriptStep::Send(vec![b"* FLAGS (\\Seen)\r\n* 1 EXISTS\r\n* OK [UIDVALIDITY 77] valid\r\n* OK [HIGHESTMODSEQ 9] modseq\r\nA0003 OK SELECT completed\r\n".to_vec()]),
                ScriptStep::ExpectContains(b"UID FETCH 42".to_vec()),
                ScriptStep::Send(vec![b"* 1 FETCH (UID 42 FLAGS () MODSEQ (9))\r\nA0004 OK FETCH completed\r\n".to_vec()]),
                ScriptStep::ExpectContains(b"UNCHANGEDSINCE 9".to_vec()),
                ScriptStep::Send(vec![b"A0005 OK STORE completed\r\n".to_vec()]),
                ScriptStep::ExpectContains(b"UID FETCH 42".to_vec()),
                ScriptStep::Send(vec![b"* 1 FETCH (UID 42 FLAGS (\\Seen) MODSEQ (10))\r\nA0006 OK FETCH completed\r\n".to_vec()]),
            ],
        )
        .await;
        let mut session = connect_at(
            &QQ_PRESET,
            "owner@qq.com",
            &SensitiveString::new("fictional-code"),
            "localhost",
            server.port(),
            certificate.roots(),
            &NeverCancelled,
        )
        .await
        .unwrap();
        session
            .select_mailbox("INBOX", &NeverCancelled)
            .await
            .unwrap();
        let before = session.fetch_flags(42, &NeverCancelled).await.unwrap();
        assert!(!before.seen);
        assert_eq!(before.modseq, Some(9));
        session
            .store_seen(42, true, before.modseq, &NeverCancelled)
            .await
            .unwrap();
        let after = session.fetch_flags(42, &NeverCancelled).await.unwrap();
        assert!(after.seen);
        assert_eq!(after.modseq, Some(10));
        let transcript = server.transcript();
        assert!(
            transcript[4]
                .windows(b"UID STORE 42 (UNCHANGEDSINCE 9) +FLAGS.SILENT (\\Seen)".len())
                .any(|window| {
                    window == b"UID STORE 42 (UNCHANGEDSINCE 9) +FLAGS.SILENT (\\Seen)"
                })
        );
        drop(session);
        server.finish().await;
    }

    #[tokio::test]
    async fn sent_reconciliation_searches_by_stable_message_id() {
        let certificate = TestCertificate::localhost();
        let server = ScriptedTlsServer::spawn_script(
            &certificate,
            vec![
                ScriptStep::Send(vec![b"* OK ready\r\n".to_vec()]),
                ScriptStep::ExpectContains(b"LOGIN".to_vec()),
                ScriptStep::Send(vec![b"A0001 OK LOGIN completed\r\n".to_vec()]),
                ScriptStep::ExpectContains(b"CAPABILITY".to_vec()),
                ScriptStep::Send(vec![b"* CAPABILITY IMAP4rev1\r\nA0002 OK completed\r\n".to_vec()]),
                ScriptStep::ExpectContains(b"SELECT".to_vec()),
                ScriptStep::Send(vec![b"* FLAGS (\\Seen)\r\n* 1 EXISTS\r\n* OK [UIDVALIDITY 88] valid\r\nA0003 OK SELECT completed\r\n".to_vec()]),
                ScriptStep::ExpectContains(
                    b"UID SEARCH HEADER Message-ID \"<send@example.test>\"".to_vec(),
                ),
                ScriptStep::Send(vec![b"* SEARCH 51\r\nA0004 OK SEARCH completed\r\n".to_vec()]),
            ],
        )
        .await;
        let mut session = connect_at(
            &QQ_PRESET,
            "owner@qq.com",
            &SensitiveString::new("fictional-code"),
            "localhost",
            server.port(),
            certificate.roots(),
            &NeverCancelled,
        )
        .await
        .unwrap();
        let selected = session
            .select_mailbox("Sent Messages", &NeverCancelled)
            .await
            .unwrap();
        assert_eq!(selected.uid_validity, 88);
        assert_eq!(
            session
                .search_message_id("<send@example.test>", &NeverCancelled)
                .await
                .unwrap(),
            vec![51]
        );
        assert_eq!(
            session
                .search_message_id("<bad\r\nid>", &NeverCancelled)
                .await
                .unwrap_err()
                .code,
            "imap_message_id_invalid"
        );
        drop(session);
        server.finish().await;
    }
}
