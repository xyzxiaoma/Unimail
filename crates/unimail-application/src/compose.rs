//! Runtime-neutral local draft creation and explicit provider submission.

use std::{fmt, sync::Arc};

use unimail_core::{
    Account, AccountAuthState, AccountId, Cancellation, CompleteOutboundAttemptInput, Draft,
    DraftAddress, DraftId, DraftSaveInput, DraftSendReviewKey, MailProvider, MessageId,
    MimeAddress, MimeBody, MimeCodec, MimeLimits, OfflineDraftReviewInput,
    OfflineDraftReviewResult, OutboundAttempt, OutboundAttemptId, OutboundAttemptOutcome,
    OutboundAttemptSnapshot, OutboundFailureCode, OutboundMessage, PrepareOutboundAttemptInput,
    Provider, ProviderError, ProviderErrorKind, ProviderFuture, ReconcileOutboundAttemptInput,
    ReconciliationKey, RecordSentRefreshInput, ReplyHeaders, ReplySource, RepositoryError,
    SendConfirmationRequired, SendOutcome, SendRequest, SentProjection, SentReconciliationRequest,
    SentReconciliationResult,
};

use crate::{Clock, StoreFuture};

/// Connectivity information available before an explicit send click is handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectivityState {
    Offline,
    AvailableOrUnknown,
}

/// Backend-generated identity values. Implementations must not use a device hostname.
#[derive(Clone, PartialEq, Eq)]
pub struct OutboundIdentity {
    pub message_id: String,
    pub date_rfc2822: String,
}

impl fmt::Debug for OutboundIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OutboundIdentity")
            .field("has_message_id", &!self.message_id.is_empty())
            .field("has_date", &!self.date_rfc2822.is_empty())
            .finish_non_exhaustive()
    }
}

/// Generates one stable Message-ID and Date immediately before exact MIME composition.
pub trait OutboundIdentityGenerator: Send + Sync {
    fn generate(&self) -> OutboundIdentity;
}

/// Asynchronous storage boundary used only by compose/send application logic.
pub trait ComposeStore: Send + Sync {
    fn get_account(&self, account_id: AccountId) -> StoreFuture<'_, Option<Account>>;
    fn get_draft(&self, draft_id: DraftId) -> StoreFuture<'_, Option<Draft>>;
    fn save_draft(&self, input: DraftSaveInput) -> StoreFuture<'_, Draft>;
    fn get_reply_source(&self, message_id: MessageId) -> StoreFuture<'_, Option<ReplySource>>;
    fn retain_offline_draft(
        &self,
        input: OfflineDraftReviewInput,
    ) -> StoreFuture<'_, OfflineDraftReviewResult>;
    fn list_send_confirmation_required(
        &self,
        account_id: Option<AccountId>,
    ) -> StoreFuture<'_, Vec<SendConfirmationRequired>>;
    fn consume_draft_send_review(&self, key: DraftSendReviewKey) -> StoreFuture<'_, bool>;
    fn prepare_outbound_attempt(
        &self,
        input: PrepareOutboundAttemptInput,
    ) -> StoreFuture<'_, OutboundAttempt>;
    fn complete_outbound_attempt(
        &self,
        input: CompleteOutboundAttemptInput,
    ) -> StoreFuture<'_, OutboundAttempt>;
    fn list_sent_projections(
        &self,
        account_id: Option<AccountId>,
    ) -> StoreFuture<'_, Vec<SentProjection>>;
    fn record_sent_refresh(&self, input: RecordSentRefreshInput) -> StoreFuture<'_, u32>;
    fn reconcile_outbound_attempt(
        &self,
        input: ReconcileOutboundAttemptInput,
    ) -> StoreFuture<'_, OutboundAttempt>;
}

/// Narrow provider submission boundary, deliberately separate from `SyncProvider`.
pub trait ExplicitSendProvider: Send + Sync {
    fn provider(&self) -> Provider;
    fn send<'a>(
        &'a self,
        request: SendRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, SendOutcome>;
}

impl<T> ExplicitSendProvider for T
where
    T: MailProvider + ?Sized,
{
    fn provider(&self) -> Provider {
        MailProvider::provider(self)
    }

    fn send<'a>(
        &'a self,
        request: SendRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, SendOutcome> {
        MailProvider::send(self, request, cancellation)
    }
}

/// Narrow read-only provider boundary for Sent reconciliation.
pub trait SentReconciliationProvider: Send + Sync {
    fn provider(&self) -> Provider;
    fn find_sent<'a>(
        &'a self,
        request: SentReconciliationRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, SentReconciliationResult>;
}

impl<T> SentReconciliationProvider for T
where
    T: MailProvider + ?Sized,
{
    fn provider(&self) -> Provider {
        MailProvider::provider(self)
    }

    fn find_sent<'a>(
        &'a self,
        request: SentReconciliationRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, SentReconciliationResult> {
        MailProvider::find_sent(self, request, cancellation)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SentReconciliationError {
    AccountUnavailable,
    Storage(RepositoryError),
    Provider(ProviderError),
}

impl From<RepositoryError> for SentReconciliationError {
    fn from(value: RepositoryError) -> Self {
        Self::Storage(value)
    }
}

/// Explicit user-triggered, read-only Sent reconciliation service.
pub struct SentReconciliationService {
    store: Arc<dyn ComposeStore>,
    provider: Arc<dyn SentReconciliationProvider>,
    clock: Arc<dyn Clock>,
}

impl SentReconciliationService {
    #[must_use]
    pub fn new(
        store: Arc<dyn ComposeStore>,
        provider: Arc<dyn SentReconciliationProvider>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            store,
            provider,
            clock,
        }
    }

    /// Queries provider Sent storage and atomically merges every matching attempt.
    ///
    /// # Errors
    ///
    /// Returns a safe typed category when the account, provider, or storage is unavailable.
    pub async fn refresh_account(
        &self,
        account_id: AccountId,
        cancellation: &dyn Cancellation,
    ) -> Result<u32, SentReconciliationError> {
        let account = self
            .store
            .get_account(account_id)
            .await?
            .filter(account_can_send)
            .filter(|account| account.provider == self.provider.provider())
            .ok_or(SentReconciliationError::AccountUnavailable)?;
        let attempts = self.store.list_sent_projections(Some(account.id)).await?;
        let mut reconciled = 0_u32;
        for projection in attempts {
            let attempt = projection.attempt;
            if !matches!(
                attempt.state,
                unimail_core::OutboundAttemptState::AcceptedPending
                    | unimail_core::OutboundAttemptState::UnknownLocked
            ) {
                continue;
            }
            let request = SentReconciliationRequest {
                account_id,
                provider_message_id: attempt.provider_message_id.clone(),
                reconciliation_key: ReconciliationKey::new(attempt.message.message_id.clone()),
            };
            match self
                .provider
                .find_sent(request, cancellation)
                .await
                .map_err(SentReconciliationError::Provider)?
            {
                SentReconciliationResult::Pending => {}
                SentReconciliationResult::Found { mailbox, message } => {
                    self.store
                        .reconcile_outbound_attempt(ReconcileOutboundAttemptInput {
                            attempt_id: attempt.id,
                            mailbox,
                            message: *message,
                            reconciled_at_ms: self.clock.now_ms().max(0),
                        })
                        .await?;
                    reconciled = reconciled.saturating_add(1);
                }
            }
        }
        let reviewed = self
            .store
            .record_sent_refresh(RecordSentRefreshInput {
                account_id,
                refreshed_at_ms: self.clock.now_ms().max(0),
            })
            .await?;
        Ok(reconciled.saturating_add(reviewed))
    }
}

/// Exact user confirmation inputs for one explicit send click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExplicitSendRequest {
    pub draft_id: DraftId,
    pub draft_revision: u64,
    pub empty_subject_confirmed: bool,
    pub offline_review_confirmed: bool,
}

/// Durable result returned after the explicit-send application service finishes.
#[derive(Clone, PartialEq, Eq)]
pub enum ExplicitSendResult {
    OfflineRetained(OfflineDraftReviewResult),
    Accepted(OutboundAttempt),
    Rejected(OutboundAttempt),
    UnknownAfterSubmission(OutboundAttempt),
}

impl fmt::Debug for ExplicitSendResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OfflineRetained(result) => formatter
                .debug_tuple("OfflineRetained")
                .field(result)
                .finish(),
            Self::Accepted(attempt) => formatter.debug_tuple("Accepted").field(attempt).finish(),
            Self::Rejected(attempt) => formatter.debug_tuple("Rejected").field(attempt).finish(),
            Self::UnknownAfterSubmission(attempt) => formatter
                .debug_tuple("UnknownAfterSubmission")
                .field(attempt)
                .finish(),
        }
    }
}

/// Safe application failure. Provider response text and mail content are never retained here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExplicitSendError {
    Storage(RepositoryError),
    DraftNotFound,
    AccountUnavailable,
    InvalidDraft,
    EmptySubjectConfirmationRequired,
    OfflineReviewConfirmationRequired,
    SendLocked,
}

impl fmt::Display for ExplicitSendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Storage(_) => "explicit send storage failure",
            Self::DraftNotFound => "draft was not found",
            Self::AccountUnavailable => "sending account is unavailable",
            Self::InvalidDraft => "draft is invalid",
            Self::EmptySubjectConfirmationRequired => "empty subject confirmation is required",
            Self::OfflineReviewConfirmationRequired => {
                "offline draft review confirmation is required"
            }
            Self::SendLocked => "draft send remains locked for review",
        })
    }
}

impl std::error::Error for ExplicitSendError {}

impl From<RepositoryError> for ExplicitSendError {
    fn from(value: RepositoryError) -> Self {
        Self::Storage(value)
    }
}

/// Explicit compose/send use case. No background trigger owns an instance of this service.
pub struct ExplicitSendService {
    store: Arc<dyn ComposeStore>,
    provider: Arc<dyn ExplicitSendProvider>,
    mime: Arc<dyn MimeCodec>,
    clock: Arc<dyn Clock>,
    identity: Arc<dyn OutboundIdentityGenerator>,
}

impl ExplicitSendService {
    #[must_use]
    pub fn new(
        store: Arc<dyn ComposeStore>,
        provider: Arc<dyn ExplicitSendProvider>,
        mime: Arc<dyn MimeCodec>,
        clock: Arc<dyn Clock>,
        identity: Arc<dyn OutboundIdentityGenerator>,
    ) -> Self {
        Self {
            store,
            provider,
            mime,
            clock,
            identity,
        }
    }

    /// Creates a local plain-text reply draft with backend-owned provider context.
    ///
    /// # Errors
    ///
    /// Returns a safe application/storage category when the source cannot be replied to.
    pub async fn create_reply_draft(
        &self,
        source_message_id: MessageId,
        draft_id: DraftId,
    ) -> Result<Draft, ExplicitSendError> {
        let source = self
            .store
            .get_reply_source(source_message_id)
            .await?
            .ok_or(ExplicitSendError::DraftNotFound)?;
        let account = self
            .store
            .get_account(source.account_id)
            .await?
            .filter(account_can_send)
            .ok_or(ExplicitSendError::AccountUnavailable)?;
        if account.provider != self.provider.provider() {
            return Err(ExplicitSendError::AccountUnavailable);
        }
        let body = quote_reply_body(source.plain_body.as_deref());
        self.store
            .save_draft(DraftSaveInput {
                id: draft_id,
                account_id: source.account_id,
                to: vec![source.sender],
                cc: Vec::new(),
                bcc: Vec::new(),
                subject: reply_subject(&source.subject),
                plain_body: body,
                html_body: None,
                in_reply_to_message_id: Some(source.message_id),
                attachments: Vec::new(),
                expected_revision: None,
                updated_at_ms: self.clock.now_ms().max(0),
            })
            .await
            .map_err(Into::into)
    }

    /// Submits one exact current draft revision only from this explicit call.
    ///
    /// # Errors
    ///
    /// Returns a safe validation/storage category. Provider failures become durable rejected state.
    pub async fn send_draft(
        &self,
        request: ExplicitSendRequest,
        connectivity: ConnectivityState,
        cancellation: &dyn Cancellation,
    ) -> Result<ExplicitSendResult, ExplicitSendError> {
        let draft = self
            .store
            .get_draft(request.draft_id)
            .await?
            .ok_or(ExplicitSendError::DraftNotFound)?;
        if draft.revision != request.draft_revision {
            return Err(ExplicitSendError::Storage(
                RepositoryError::RevisionConflict,
            ));
        }
        validate_draft(&draft, request.empty_subject_confirmed)?;
        let account = self
            .store
            .get_account(draft.account_id)
            .await?
            .filter(account_can_send)
            .ok_or(ExplicitSendError::AccountUnavailable)?;
        if account.provider != self.provider.provider() {
            return Err(ExplicitSendError::AccountUnavailable);
        }
        let confirmations = self
            .store
            .list_send_confirmation_required(Some(draft.account_id))
            .await?;
        let offline_confirmation = confirmations.iter().any(|confirmation| {
            confirmation.draft_id == draft.id && confirmation.draft_revision == draft.revision
        });
        if offline_confirmation && !request.offline_review_confirmed {
            return Err(ExplicitSendError::OfflineReviewConfirmationRequired);
        }
        let now_ms = self.clock.now_ms().max(0);
        if connectivity == ConnectivityState::Offline {
            return self.retain_offline(draft, now_ms).await;
        }
        if offline_confirmation {
            let consumed = self
                .store
                .consume_draft_send_review(DraftSendReviewKey {
                    draft_id: draft.id,
                    draft_revision: draft.revision,
                })
                .await?;
            if !consumed {
                return Err(ExplicitSendError::Storage(
                    RepositoryError::RevisionConflict,
                ));
            }
        }
        let prepared = self.prepare_attempt(draft, account, now_ms).await?;
        let attempt_id = prepared.id;
        let send_request = SendRequest {
            account_id: prepared.account_id,
            provider_thread_id: prepared.provider_thread_id.clone(),
            original_provider_message_id: prepared.original_provider_message_id.clone(),
            message: prepared.message.clone(),
        };
        let outcome = classify_send_outcome(
            self.provider.send(send_request, cancellation).await,
            &prepared.message.message_id,
        );
        let completed = self
            .store
            .complete_outbound_attempt(CompleteOutboundAttemptInput {
                attempt_id,
                outcome: outcome.clone(),
                updated_at_ms: self.clock.now_ms().max(now_ms),
            })
            .await?;
        Ok(match outcome {
            OutboundAttemptOutcome::Accepted { .. } => ExplicitSendResult::Accepted(completed),
            OutboundAttemptOutcome::Rejected { .. } => ExplicitSendResult::Rejected(completed),
            OutboundAttemptOutcome::UnknownAfterSubmission => {
                ExplicitSendResult::UnknownAfterSubmission(completed)
            }
        })
    }

    async fn prepare_attempt(
        &self,
        draft: Draft,
        account: Account,
        now_ms: i64,
    ) -> Result<OutboundAttempt, ExplicitSendError> {
        let reply_source = if let Some(message_id) = draft.in_reply_to_message_id {
            Some(
                self.store
                    .get_reply_source(message_id)
                    .await?
                    .filter(|source| source.account_id == draft.account_id)
                    .ok_or(ExplicitSendError::InvalidDraft)?,
            )
        } else {
            None
        };
        let identity = self.identity.generate();
        let outbound = build_outbound_message(&draft, &account, reply_source.as_ref(), &identity);
        let composed = self
            .mime
            .compose(&outbound, MimeLimits::default())
            .map_err(|_| ExplicitSendError::InvalidDraft)?;
        self.store
            .prepare_outbound_attempt(PrepareOutboundAttemptInput {
                id: OutboundAttemptId::new(),
                draft_id: draft.id,
                draft_revision: draft.revision,
                account_id: draft.account_id,
                in_reply_to_message_id: draft.in_reply_to_message_id,
                provider_thread_id: reply_source
                    .as_ref()
                    .and_then(|source| source.provider_thread_id.clone()),
                original_provider_message_id: reply_source
                    .as_ref()
                    .map(|source| source.original_provider_message_id.clone()),
                date_rfc2822: identity.date_rfc2822,
                message: composed,
                snapshot: OutboundAttemptSnapshot {
                    sender: DraftAddress {
                        display_name: account.display_name,
                        address: account.email,
                    },
                    to: draft.to,
                    cc: draft.cc,
                    bcc: draft.bcc,
                    subject: draft.subject,
                    plain_body: draft.plain_body,
                },
                created_at_ms: now_ms,
            })
            .await
            .map_err(|error| {
                if error == RepositoryError::ConstraintViolation {
                    ExplicitSendError::SendLocked
                } else {
                    ExplicitSendError::Storage(error)
                }
            })
    }

    async fn retain_offline(
        &self,
        draft: Draft,
        now_ms: i64,
    ) -> Result<ExplicitSendResult, ExplicitSendError> {
        let retained = self
            .store
            .retain_offline_draft(OfflineDraftReviewInput {
                draft: DraftSaveInput {
                    id: draft.id,
                    account_id: draft.account_id,
                    to: draft.to,
                    cc: draft.cc,
                    bcc: draft.bcc,
                    subject: draft.subject,
                    plain_body: draft.plain_body,
                    html_body: None,
                    in_reply_to_message_id: draft.in_reply_to_message_id,
                    attachments: Vec::new(),
                    expected_revision: Some(draft.revision),
                    updated_at_ms: now_ms,
                },
                reviewed_at_ms: now_ms,
            })
            .await?;
        Ok(ExplicitSendResult::OfflineRetained(retained))
    }
}

fn account_can_send(account: &Account) -> bool {
    account.enabled && !account.deleting && account.auth_state == AccountAuthState::Connected
}

fn validate_draft(draft: &Draft, empty_subject_confirmed: bool) -> Result<(), ExplicitSendError> {
    if draft.html_body.is_some() || !draft.attachments.is_empty() {
        return Err(ExplicitSendError::InvalidDraft);
    }
    let recipients = draft.to.iter().chain(&draft.cc).chain(&draft.bcc);
    if recipients.clone().next().is_none()
        || recipients.clone().any(invalid_address)
        || (draft.subject.trim().is_empty() && draft.plain_body.trim().is_empty())
    {
        return Err(ExplicitSendError::InvalidDraft);
    }
    if draft.subject.trim().is_empty() && !empty_subject_confirmed {
        return Err(ExplicitSendError::EmptySubjectConfirmationRequired);
    }
    Ok(())
}

fn invalid_address(address: &DraftAddress) -> bool {
    let value = address.address.trim();
    value.is_empty()
        || value.contains(['\r', '\n'])
        || value.split_once('@').is_none_or(|(local, domain)| {
            local.is_empty()
                || domain.is_empty()
                || domain.starts_with('.')
                || domain.ends_with('.')
                || domain.contains('@')
        })
}

fn build_outbound_message(
    draft: &Draft,
    account: &Account,
    reply_source: Option<&ReplySource>,
    identity: &OutboundIdentity,
) -> OutboundMessage {
    let to = draft.to.iter().map(to_mime_address).collect::<Vec<_>>();
    let cc = draft.cc.iter().map(to_mime_address).collect::<Vec<_>>();
    let mut envelope_recipients = draft
        .to
        .iter()
        .chain(&draft.cc)
        .chain(&draft.bcc)
        .map(|address| address.address.trim().to_owned())
        .collect::<Vec<_>>();
    envelope_recipients.sort_by_key(|value| value.to_ascii_lowercase());
    envelope_recipients.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    let mut references = reply_source.map_or_else(Vec::new, |source| source.references.clone());
    let in_reply_to = reply_source.and_then(|source| source.rfc_message_id.clone());
    if let Some(message_id) = &in_reply_to
        && !references.iter().any(|reference| reference == message_id)
    {
        references.push(message_id.clone());
    }
    OutboundMessage {
        message_id: identity.message_id.clone(),
        date_rfc2822: identity.date_rfc2822.clone(),
        from: MimeAddress {
            display_name: account.display_name.clone(),
            address: account.email.clone(),
        },
        sender: None,
        reply_to: Vec::new(),
        to,
        cc,
        subject: draft.subject.clone(),
        body: MimeBody {
            plain: Some(draft.plain_body.clone()),
            html: None,
        },
        reply: ReplyHeaders {
            in_reply_to,
            references,
        },
        attachments: Vec::new(),
        envelope: unimail_core::DeliveryEnvelope {
            from: account.email.clone(),
            recipients: envelope_recipients,
        },
    }
}

fn to_mime_address(address: &DraftAddress) -> MimeAddress {
    MimeAddress {
        display_name: address.display_name.clone(),
        address: address.address.trim().to_owned(),
    }
}

fn classify_send_outcome(
    result: Result<SendOutcome, ProviderError>,
    expected_message_id: &str,
) -> OutboundAttemptOutcome {
    match result {
        Ok(SendOutcome::Accepted(accepted))
            if accepted.reconciliation_key.expose() == expected_message_id =>
        {
            OutboundAttemptOutcome::Accepted {
                provider_message_id: accepted.provider_message_id,
            }
        }
        Ok(SendOutcome::Accepted(_) | SendOutcome::UnknownAfterSubmission(_)) => {
            OutboundAttemptOutcome::UnknownAfterSubmission
        }
        Ok(SendOutcome::Rejected(_)) => OutboundAttemptOutcome::Rejected {
            safe_error_code: OutboundFailureCode::RecipientRejected,
        },
        Err(error) => OutboundAttemptOutcome::Rejected {
            safe_error_code: provider_error_code(error.kind),
        },
    }
}

fn provider_error_code(kind: ProviderErrorKind) -> OutboundFailureCode {
    match kind {
        ProviderErrorKind::Authentication | ProviderErrorKind::Permission => {
            OutboundFailureCode::AuthenticationRequired
        }
        ProviderErrorKind::Transient | ProviderErrorKind::Throttled => {
            OutboundFailureCode::ProviderUnavailable
        }
        ProviderErrorKind::Cancelled
        | ProviderErrorKind::InvalidCursor
        | ProviderErrorKind::Protocol
        | ProviderErrorKind::Permanent => OutboundFailureCode::Internal,
    }
}

fn reply_subject(subject: &str) -> String {
    let trimmed = subject.trim();
    if trimmed
        .get(..3)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("re:"))
    {
        trimmed.to_owned()
    } else if trimmed.is_empty() {
        "Re:".to_owned()
    } else {
        format!("Re: {trimmed}")
    }
}

fn quote_reply_body(plain_body: Option<&str>) -> String {
    let source = plain_body
        .filter(|body| !body.trim().is_empty())
        .unwrap_or("（原邮件仅包含 HTML 内容，请在阅读区查看。）");
    let quoted = source
        .lines()
        .map(|line| format!("> {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("\n\n在原邮件中写道：\n{quoted}")
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use unimail_core::{
        AcceptedSend, AccountAuthState, CancellationFuture, DraftSendReview, DraftSendReviewReason,
        MailboxRole, NormalizedMimeMessage, ProviderError, ReconciliationKey, RejectedSend,
        RemoteMailbox, RemoteMailboxKey, RemoteMessage, RemoteMessageKey, UnknownSend,
    };
    use unimail_providers::SharedMimeCodec;

    use crate::test_support::block_on;

    use super::*;

    #[derive(Clone)]
    struct FakeStore {
        state: Arc<Mutex<FakeStoreState>>,
    }

    struct FakeStoreState {
        account: Account,
        draft: Option<Draft>,
        reply_source: Option<ReplySource>,
        confirmations: Vec<SendConfirmationRequired>,
        prepared: Vec<OutboundAttempt>,
        completed: Vec<CompleteOutboundAttemptInput>,
        offline_count: u32,
    }

    impl FakeStore {
        fn new(account: Account, draft: Draft) -> Self {
            Self {
                state: Arc::new(Mutex::new(FakeStoreState {
                    account,
                    draft: Some(draft),
                    reply_source: None,
                    confirmations: Vec::new(),
                    prepared: Vec::new(),
                    completed: Vec::new(),
                    offline_count: 0,
                })),
            }
        }
    }

    impl ComposeStore for FakeStore {
        fn get_account(&self, account_id: AccountId) -> StoreFuture<'_, Option<Account>> {
            let value = self.state.lock().expect("fake store").account.clone();
            Box::pin(async move { Ok((value.id == account_id).then_some(value)) })
        }

        fn get_draft(&self, draft_id: DraftId) -> StoreFuture<'_, Option<Draft>> {
            let value = self.state.lock().expect("fake store").draft.clone();
            Box::pin(async move { Ok(value.filter(|draft| draft.id == draft_id)) })
        }

        fn save_draft(&self, input: DraftSaveInput) -> StoreFuture<'_, Draft> {
            let state = Arc::clone(&self.state);
            Box::pin(async move {
                let mut state = state.lock().expect("fake store");
                let revision = input.expected_revision.map_or(1, |revision| revision + 1);
                let draft = Draft {
                    id: input.id,
                    account_id: input.account_id,
                    to: input.to,
                    cc: input.cc,
                    bcc: input.bcc,
                    subject: input.subject,
                    plain_body: input.plain_body,
                    html_body: input.html_body,
                    in_reply_to_message_id: input.in_reply_to_message_id,
                    attachments: input.attachments,
                    revision,
                    created_at_ms: input.updated_at_ms,
                    updated_at_ms: input.updated_at_ms,
                };
                state.draft = Some(draft.clone());
                Ok(draft)
            })
        }

        fn get_reply_source(&self, message_id: MessageId) -> StoreFuture<'_, Option<ReplySource>> {
            let value = self.state.lock().expect("fake store").reply_source.clone();
            Box::pin(async move { Ok(value.filter(|source| source.message_id == message_id)) })
        }

        fn retain_offline_draft(
            &self,
            input: OfflineDraftReviewInput,
        ) -> StoreFuture<'_, OfflineDraftReviewResult> {
            let state = Arc::clone(&self.state);
            Box::pin(async move {
                let mut state = state.lock().expect("fake store");
                state.offline_count += 1;
                let revision = input
                    .draft
                    .expected_revision
                    .map_or(1, |revision| revision + 1);
                let draft = Draft {
                    id: input.draft.id,
                    account_id: input.draft.account_id,
                    to: input.draft.to,
                    cc: input.draft.cc,
                    bcc: input.draft.bcc,
                    subject: input.draft.subject,
                    plain_body: input.draft.plain_body,
                    html_body: input.draft.html_body,
                    in_reply_to_message_id: input.draft.in_reply_to_message_id,
                    attachments: input.draft.attachments,
                    revision,
                    created_at_ms: input.draft.updated_at_ms,
                    updated_at_ms: input.draft.updated_at_ms,
                };
                let review = DraftSendReview {
                    draft_id: draft.id,
                    account_id: draft.account_id,
                    draft_revision: revision,
                    reason: DraftSendReviewReason::Offline,
                    created_at_ms: input.reviewed_at_ms,
                    updated_at_ms: input.reviewed_at_ms,
                };
                state.draft = Some(draft.clone());
                Ok(OfflineDraftReviewResult { draft, review })
            })
        }

        fn list_send_confirmation_required(
            &self,
            _account_id: Option<AccountId>,
        ) -> StoreFuture<'_, Vec<SendConfirmationRequired>> {
            let value = self.state.lock().expect("fake store").confirmations.clone();
            Box::pin(async move { Ok(value) })
        }

        fn consume_draft_send_review(&self, key: DraftSendReviewKey) -> StoreFuture<'_, bool> {
            let state = Arc::clone(&self.state);
            Box::pin(async move {
                let mut state = state.lock().expect("fake store");
                let before = state.confirmations.len();
                state.confirmations.retain(|confirmation| {
                    confirmation.draft_id != key.draft_id
                        || confirmation.draft_revision != key.draft_revision
                });
                Ok(before != state.confirmations.len())
            })
        }

        fn prepare_outbound_attempt(
            &self,
            input: PrepareOutboundAttemptInput,
        ) -> StoreFuture<'_, OutboundAttempt> {
            let state = Arc::clone(&self.state);
            Box::pin(async move {
                let attempt = OutboundAttempt {
                    id: input.id,
                    draft_id: input.draft_id,
                    draft_revision: input.draft_revision,
                    account_id: input.account_id,
                    in_reply_to_message_id: input.in_reply_to_message_id,
                    provider_thread_id: input.provider_thread_id,
                    original_provider_message_id: input.original_provider_message_id,
                    date_rfc2822: input.date_rfc2822,
                    message: input.message,
                    snapshot: input.snapshot,
                    state: unimail_core::OutboundAttemptState::Submitting,
                    provider_message_id: None,
                    reconciled_message_id: None,
                    safe_error_code: None,
                    sent_refresh_count: 0,
                    retry_authorized: false,
                    created_at_ms: input.created_at_ms,
                    updated_at_ms: input.created_at_ms,
                };
                state
                    .lock()
                    .expect("fake store")
                    .prepared
                    .push(attempt.clone());
                Ok(attempt)
            })
        }

        fn complete_outbound_attempt(
            &self,
            input: CompleteOutboundAttemptInput,
        ) -> StoreFuture<'_, OutboundAttempt> {
            let state = Arc::clone(&self.state);
            Box::pin(async move {
                let mut state = state.lock().expect("fake store");
                let mut attempt = state
                    .prepared
                    .iter()
                    .find(|attempt| attempt.id == input.attempt_id)
                    .cloned()
                    .ok_or(RepositoryError::NotFound)?;
                match &input.outcome {
                    OutboundAttemptOutcome::Accepted {
                        provider_message_id,
                    } => {
                        attempt.state = unimail_core::OutboundAttemptState::AcceptedPending;
                        attempt.provider_message_id.clone_from(provider_message_id);
                        state.draft = None;
                    }
                    OutboundAttemptOutcome::Rejected { safe_error_code } => {
                        attempt.state = unimail_core::OutboundAttemptState::Rejected;
                        attempt.safe_error_code = Some(*safe_error_code);
                    }
                    OutboundAttemptOutcome::UnknownAfterSubmission => {
                        attempt.state = unimail_core::OutboundAttemptState::UnknownLocked;
                    }
                }
                attempt.updated_at_ms = input.updated_at_ms;
                if let Some(stored) = state
                    .prepared
                    .iter_mut()
                    .find(|stored| stored.id == attempt.id)
                {
                    stored.clone_from(&attempt);
                }
                state.completed.push(input);
                Ok(attempt)
            })
        }

        fn list_sent_projections(
            &self,
            _account_id: Option<AccountId>,
        ) -> StoreFuture<'_, Vec<SentProjection>> {
            let attempts = self
                .state
                .lock()
                .expect("fake store")
                .prepared
                .iter()
                .filter(|attempt| {
                    matches!(
                        attempt.state,
                        unimail_core::OutboundAttemptState::AcceptedPending
                            | unimail_core::OutboundAttemptState::Reconciled
                            | unimail_core::OutboundAttemptState::UnknownLocked
                    )
                })
                .cloned()
                .map(|attempt| SentProjection { attempt })
                .collect();
            Box::pin(async move { Ok(attempts) })
        }

        fn record_sent_refresh(&self, input: RecordSentRefreshInput) -> StoreFuture<'_, u32> {
            let state = Arc::clone(&self.state);
            Box::pin(async move {
                let mut state = state.lock().expect("fake store");
                let mut updated = 0_u32;
                for attempt in &mut state.prepared {
                    if attempt.account_id == input.account_id
                        && matches!(
                            attempt.state,
                            unimail_core::OutboundAttemptState::AcceptedPending
                                | unimail_core::OutboundAttemptState::UnknownLocked
                        )
                    {
                        attempt.sent_refresh_count = attempt.sent_refresh_count.saturating_add(1);
                        updated = updated.saturating_add(1);
                    }
                }
                Ok(updated)
            })
        }

        fn reconcile_outbound_attempt(
            &self,
            input: ReconcileOutboundAttemptInput,
        ) -> StoreFuture<'_, OutboundAttempt> {
            let state = Arc::clone(&self.state);
            Box::pin(async move {
                let mut state = state.lock().expect("fake store");
                let attempt = state
                    .prepared
                    .iter_mut()
                    .find(|attempt| attempt.id == input.attempt_id)
                    .ok_or(RepositoryError::NotFound)?;
                attempt.state = unimail_core::OutboundAttemptState::Reconciled;
                attempt.reconciled_message_id = Some(MessageId::new());
                attempt.updated_at_ms = input.reconciled_at_ms;
                Ok(attempt.clone())
            })
        }
    }

    struct FakeProvider {
        outcome: Mutex<Option<Result<SendOutcome, ProviderError>>>,
        calls: AtomicUsize,
        requests: Mutex<Vec<SendRequest>>,
    }

    impl FakeProvider {
        fn new(outcome: Result<SendOutcome, ProviderError>) -> Self {
            Self {
                outcome: Mutex::new(Some(outcome)),
                calls: AtomicUsize::new(0),
                requests: Mutex::new(Vec::new()),
            }
        }
    }

    impl ExplicitSendProvider for FakeProvider {
        fn provider(&self) -> Provider {
            Provider::Gmail
        }

        fn send<'a>(
            &'a self,
            request: SendRequest,
            _cancellation: &'a dyn Cancellation,
        ) -> ProviderFuture<'a, SendOutcome> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.requests.lock().expect("requests").push(request);
            let outcome = self.outcome.lock().expect("outcome").take();
            Box::pin(async move {
                outcome.unwrap_or({
                    Ok(SendOutcome::Rejected(RejectedSend {
                        code: "fictional_duplicate_call",
                    }))
                })
            })
        }
    }

    struct FakeSentProvider {
        result: Mutex<Option<Result<SentReconciliationResult, ProviderError>>>,
        calls: AtomicUsize,
    }

    impl SentReconciliationProvider for FakeSentProvider {
        fn provider(&self) -> Provider {
            Provider::Gmail
        }

        fn find_sent<'a>(
            &'a self,
            _request: SentReconciliationRequest,
            _cancellation: &'a dyn Cancellation,
        ) -> ProviderFuture<'a, SentReconciliationResult> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let result = self.result.lock().expect("sent result").take();
            Box::pin(async move { result.unwrap_or(Ok(SentReconciliationResult::Pending)) })
        }
    }

    struct FixedClock;

    impl Clock for FixedClock {
        fn now_ms(&self) -> i64 {
            42
        }
    }

    struct FixedIdentity;

    impl OutboundIdentityGenerator for FixedIdentity {
        fn generate(&self) -> OutboundIdentity {
            OutboundIdentity {
                message_id: "<stable@unimail.invalid>".to_owned(),
                date_rfc2822: "Wed, 22 Jul 2026 12:00:00 +0800".to_owned(),
            }
        }
    }

    struct NeverCancelled;

    impl Cancellation for NeverCancelled {
        fn is_cancelled(&self) -> bool {
            false
        }

        fn cancelled(&self) -> CancellationFuture<'_> {
            Box::pin(std::future::pending())
        }
    }

    fn account() -> Account {
        Account {
            id: AccountId::new(),
            provider: Provider::Gmail,
            email: "owner@example.test".to_owned(),
            display_name: Some("Owner".to_owned()),
            credential_ref: unimail_core::CredentialRef::new("compose-test"),
            auth_state: AccountAuthState::Connected,
            enabled: true,
            deleting: false,
            created_at_ms: 1,
            updated_at_ms: 1,
            last_error_code: None,
        }
    }

    fn draft(account_id: AccountId) -> Draft {
        Draft {
            id: DraftId::new(),
            account_id,
            to: vec![DraftAddress {
                display_name: Some("Recipient".to_owned()),
                address: "recipient@example.test".to_owned(),
            }],
            cc: Vec::new(),
            bcc: vec![DraftAddress {
                display_name: None,
                address: "hidden@example.test".to_owned(),
            }],
            subject: "Subject".to_owned(),
            plain_body: "Body".to_owned(),
            html_body: None,
            in_reply_to_message_id: None,
            attachments: Vec::new(),
            revision: 1,
            created_at_ms: 2,
            updated_at_ms: 2,
        }
    }

    fn service(store: FakeStore, provider: Arc<FakeProvider>) -> ExplicitSendService {
        ExplicitSendService::new(
            Arc::new(store),
            provider,
            Arc::new(SharedMimeCodec),
            Arc::new(FixedClock),
            Arc::new(FixedIdentity),
        )
    }

    #[test]
    fn offline_click_retains_latest_revision_without_provider_call() {
        let account = account();
        let draft = draft(account.id);
        let store = FakeStore::new(account, draft.clone());
        let provider = Arc::new(FakeProvider::new(Ok(SendOutcome::Rejected(RejectedSend {
            code: "should_not_send",
        }))));
        let service = service(store.clone(), Arc::clone(&provider));

        let result = block_on(service.send_draft(
            ExplicitSendRequest {
                draft_id: draft.id,
                draft_revision: 1,
                empty_subject_confirmed: false,
                offline_review_confirmed: false,
            },
            ConnectivityState::Offline,
            &NeverCancelled,
        ))
        .expect("offline retention");

        assert!(matches!(result, ExplicitSendResult::OfflineRetained(_)));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        let state = store.state.lock().expect("fake store");
        assert_eq!(state.offline_count, 1);
        assert!(state.prepared.is_empty());
    }

    #[test]
    fn accepted_send_uses_shared_mime_and_keeps_bcc_envelope_only() {
        let account = account();
        let draft = draft(account.id);
        let store = FakeStore::new(account, draft.clone());
        let provider = Arc::new(FakeProvider::new(Ok(SendOutcome::Accepted(AcceptedSend {
            provider_message_id: Some("provider-sent".to_owned()),
            reconciliation_key: ReconciliationKey::new("stable@unimail.invalid"),
        }))));
        let service = service(store.clone(), Arc::clone(&provider));

        let result = block_on(service.send_draft(
            ExplicitSendRequest {
                draft_id: draft.id,
                draft_revision: 1,
                empty_subject_confirmed: false,
                offline_review_confirmed: false,
            },
            ConnectivityState::AvailableOrUnknown,
            &NeverCancelled,
        ))
        .expect("accepted send");

        assert!(matches!(result, ExplicitSendResult::Accepted(_)));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        let requests = provider.requests.lock().expect("requests");
        let request = &requests[0];
        let raw = String::from_utf8_lossy(request.message.as_bytes()).to_ascii_lowercase();
        assert!(!raw.contains("bcc:"));
        assert!(!raw.contains("hidden@example.test"));
        assert!(
            request
                .message
                .envelope
                .recipients
                .iter()
                .any(|value| value == "hidden@example.test")
        );
        assert_eq!(store.state.lock().expect("fake store").completed.len(), 1);
    }

    #[test]
    fn manual_sent_refresh_is_read_only_and_reconciles_provider_observation() {
        let account = account();
        let draft = draft(account.id);
        let store = FakeStore::new(account.clone(), draft.clone());
        let send_provider = Arc::new(FakeProvider::new(Ok(SendOutcome::Accepted(AcceptedSend {
            provider_message_id: None,
            reconciliation_key: ReconciliationKey::new("stable@unimail.invalid"),
        }))));
        let send_service = service(store.clone(), Arc::clone(&send_provider));
        block_on(send_service.send_draft(
            ExplicitSendRequest {
                draft_id: draft.id,
                draft_revision: draft.revision,
                empty_subject_confirmed: false,
                offline_review_confirmed: false,
            },
            ConnectivityState::AvailableOrUnknown,
            &NeverCancelled,
        ))
        .expect("accepted send");
        let lookup_provider = Arc::new(FakeSentProvider {
            result: Mutex::new(Some(Ok(SentReconciliationResult::Found {
                mailbox: RemoteMailbox {
                    key: RemoteMailboxKey {
                        account_id: account.id,
                        provider_mailbox_id: "sent".to_owned(),
                    },
                    role: MailboxRole::Sent,
                    display_name: "已发送".to_owned(),
                },
                message: Box::new(RemoteMessage {
                    key: RemoteMessageKey {
                        account_id: account.id,
                        provider_mailbox_id: "sent".to_owned(),
                        provider_message_id: "provider-sent".to_owned(),
                    },
                    provider_revision: None,
                    provider_thread_id: None,
                    read: true,
                    sent_at_ms: Some(42),
                    received_at_ms: 42,
                    mime: NormalizedMimeMessage {
                        subject: Some("Subject".to_owned()),
                        message_id: Some("<stable@unimail.invalid>".to_owned()),
                        in_reply_to: None,
                        references: Vec::new(),
                        addresses: Vec::new(),
                        body: MimeBody {
                            plain: Some("Body".to_owned()),
                            html: None,
                        },
                        attachments: Vec::new(),
                    },
                }),
            }))),
            calls: AtomicUsize::new(0),
        });
        let reconciliation = SentReconciliationService::new(
            Arc::new(store.clone()),
            lookup_provider.clone(),
            Arc::new(FixedClock),
        );

        assert_eq!(
            block_on(reconciliation.refresh_account(account.id, &NeverCancelled))
                .expect("refresh Sent"),
            1
        );
        assert_eq!(send_provider.calls.load(Ordering::SeqCst), 1);
        assert_eq!(lookup_provider.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            store.state.lock().expect("fake store").prepared[0].state,
            unimail_core::OutboundAttemptState::Reconciled
        );
    }

    #[test]
    fn provider_unknown_remains_terminal_and_reply_creation_uses_original_account() {
        let account = account();
        let mut draft = draft(account.id);
        let source_id = MessageId::new();
        draft.in_reply_to_message_id = Some(source_id);
        let store = FakeStore::new(account.clone(), draft.clone());
        store.state.lock().expect("fake store").reply_source = Some(ReplySource {
            message_id: source_id,
            account_id: account.id,
            provider_thread_id: Some("thread-safe".to_owned()),
            original_provider_message_id: "provider-original".to_owned(),
            rfc_message_id: Some("<original@example.test>".to_owned()),
            references: vec!["<older@example.test>".to_owned()],
            sender: DraftAddress {
                display_name: Some("Original Sender".to_owned()),
                address: "sender@example.test".to_owned(),
            },
            subject: "Topic".to_owned(),
            plain_body: Some("Original body".to_owned()),
            received_at_ms: 1,
        });
        let provider = Arc::new(FakeProvider::new(Ok(SendOutcome::UnknownAfterSubmission(
            UnknownSend {
                reconciliation_key: ReconciliationKey::new("stable@unimail.invalid"),
            },
        ))));
        let service = service(store.clone(), provider);

        let result = block_on(service.send_draft(
            ExplicitSendRequest {
                draft_id: draft.id,
                draft_revision: 1,
                empty_subject_confirmed: false,
                offline_review_confirmed: false,
            },
            ConnectivityState::AvailableOrUnknown,
            &NeverCancelled,
        ))
        .expect("unknown send");
        assert!(matches!(
            result,
            ExplicitSendResult::UnknownAfterSubmission(_)
        ));

        let reply = block_on(service.create_reply_draft(source_id, DraftId::new()))
            .expect("create reply draft");
        assert_eq!(reply.account_id, account.id);
        assert_eq!(reply.to[0].address, "sender@example.test");
        assert_eq!(reply.subject, "Re: Topic");
        assert!(reply.plain_body.contains("> Original body"));
    }

    #[test]
    fn validation_requires_empty_subject_confirmation_and_rejects_empty_message() {
        let account = account();
        let mut value = draft(account.id);
        value.subject.clear();
        let store = FakeStore::new(account, value.clone());
        let provider = Arc::new(FakeProvider::new(Ok(SendOutcome::Accepted(AcceptedSend {
            provider_message_id: None,
            reconciliation_key: ReconciliationKey::new("stable@unimail.invalid"),
        }))));
        let service = service(store.clone(), provider);
        assert_eq!(
            block_on(service.send_draft(
                ExplicitSendRequest {
                    draft_id: value.id,
                    draft_revision: 1,
                    empty_subject_confirmed: false,
                    offline_review_confirmed: false,
                },
                ConnectivityState::AvailableOrUnknown,
                &NeverCancelled,
            ))
            .expect_err("subject confirmation"),
            ExplicitSendError::EmptySubjectConfirmationRequired
        );

        store
            .state
            .lock()
            .expect("fake store")
            .draft
            .as_mut()
            .expect("draft")
            .plain_body
            .clear();
        assert_eq!(
            block_on(service.send_draft(
                ExplicitSendRequest {
                    draft_id: value.id,
                    draft_revision: 1,
                    empty_subject_confirmed: true,
                    offline_review_confirmed: false,
                },
                ConnectivityState::AvailableOrUnknown,
                &NeverCancelled,
            ))
            .expect_err("empty message"),
            ExplicitSendError::InvalidDraft
        );
    }
}
