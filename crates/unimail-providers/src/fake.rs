//! Deterministic, stateful provider and authentication fakes.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    task::Waker,
};

use unimail_core::{
    AcceptedSend, AccountAuthenticator, AttachmentDownload, AttachmentRequest, AttachmentSink,
    AuthenticatedAccount, AuthorizationCodeLoginRequest, Cancellation, CancellationFuture,
    CompleteLoginRequest, CredentialRef, DurableCheckpoint, FetchBodyRequest,
    IncrementalSyncRequest, InitialSyncRequest, LoginStart, MailProvider, NormalizedMimeMessage,
    OpaqueProviderCursor, PageContinuation, Provider, ProviderError, ProviderErrorKind,
    ProviderFuture, ProviderResult, ProviderRevision, ReadStateAck, ReconciliationKey,
    RemoteChange, RemoteMailbox, RemoteMessage, RemoteMessageKey, SendOutcome, SendRequest,
    SensitiveString, SetReadRequest, StartLoginRequest, SyncPage, SyncPageState,
};

/// Operations that can receive one-shot injected failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FakeOperation {
    InitialSync,
    IncrementalSync,
    FetchBody,
    FetchAttachment,
    SetRead,
    Send,
    StartLogin,
    CompleteLogin,
    AuthorizationCodeLogin,
    Refresh,
    Revoke,
}

/// Non-sensitive call metadata suitable for assertions and diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FakeCall {
    InitialSync {
        limit: u16,
        continued: bool,
    },
    IncrementalSync {
        continued: bool,
    },
    FetchBody,
    FetchAttachment,
    SetRead {
        desired_read: bool,
    },
    Send {
        byte_count: usize,
        recipient_count: usize,
        provider_thread_present: bool,
    },
    StartLogin {
        provider: Provider,
    },
    CompleteLogin,
    AuthorizationCodeLogin {
        provider: Provider,
    },
    Refresh,
    Revoke,
}

/// Cooperative cancellation token for fake calls and contract tests.
#[derive(Clone, Default)]
pub struct FakeCancellation {
    inner: Arc<FakeCancellationInner>,
}

#[derive(Default)]
struct FakeCancellationInner {
    cancelled: AtomicBool,
    waiters: Mutex<Vec<Waker>>,
}

impl FakeCancellation {
    /// Cancels the token and wakes every registered waiter.
    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::Release);
        if let Ok(mut waiters) = self.inner.waiters.lock() {
            for waiter in waiters.drain(..) {
                waiter.wake();
            }
        }
    }
}

impl Cancellation for FakeCancellation {
    fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    fn cancelled(&self) -> CancellationFuture<'_> {
        Box::pin(std::future::poll_fn(|context| {
            if self.is_cancelled() {
                return std::task::Poll::Ready(());
            }

            if let Ok(mut waiters) = self.inner.waiters.lock()
                && !waiters
                    .iter()
                    .any(|waiter| waiter.will_wake(context.waker()))
            {
                waiters.push(context.waker().clone());
            }
            std::task::Poll::Pending
        }))
    }
}

/// Stateful in-memory provider used by adapter-independent contract tests.
#[derive(Clone)]
pub struct FakeMailProvider {
    provider: Provider,
    state: Arc<Mutex<FakeProviderState>>,
}

struct SequencedChange {
    sequence: u64,
    change: RemoteChange,
}

struct FakeProviderState {
    mailboxes: Vec<RemoteMailbox>,
    changes: Vec<SequencedChange>,
    messages: HashMap<RemoteMessageKey, RemoteMessage>,
    attachments: HashMap<(RemoteMessageKey, String), Vec<Vec<u8>>>,
    page_size: usize,
    invalid_before: Option<u64>,
    duplicate_next_page: bool,
    failures: HashMap<FakeOperation, VecDeque<ProviderError>>,
    send_outcomes: VecDeque<SendOutcome>,
    calls: Vec<FakeCall>,
}

impl FakeMailProvider {
    /// Creates an empty provider with the requested deterministic page size.
    #[must_use]
    pub fn new(provider: Provider, page_size: usize) -> Self {
        Self {
            provider,
            state: Arc::new(Mutex::new(FakeProviderState {
                mailboxes: Vec::new(),
                changes: Vec::new(),
                messages: HashMap::new(),
                attachments: HashMap::new(),
                page_size: page_size.max(1),
                invalid_before: None,
                duplicate_next_page: false,
                failures: HashMap::new(),
                send_outcomes: VecDeque::new(),
                calls: Vec::new(),
            })),
        }
    }

    /// Adds mailbox metadata returned with every sync page.
    ///
    /// # Errors
    ///
    /// Returns a fixed fake-state error if the shared state is unavailable.
    pub fn add_mailbox(&self, mailbox: RemoteMailbox) -> ProviderResult<()> {
        self.lock_state()?.mailboxes.push(mailbox);
        Ok(())
    }

    /// Appends a normalized remote change to the monotonic fake timeline.
    ///
    /// # Errors
    ///
    /// Returns a fixed fake-state error if the shared state is unavailable.
    pub fn push_change(&self, change: RemoteChange) -> ProviderResult<u64> {
        let mut state = self.lock_state()?;
        let sequence = next_sequence(&state);
        apply_change_to_messages(&mut state.messages, &change);
        state.changes.push(SequencedChange { sequence, change });
        Ok(sequence)
    }

    /// Configures attachment chunks without retaining a destination path.
    ///
    /// # Errors
    ///
    /// Returns a fixed fake-state error if the shared state is unavailable.
    pub fn set_attachment(
        &self,
        request: AttachmentRequest,
        chunks: Vec<Vec<u8>>,
    ) -> ProviderResult<()> {
        self.lock_state()?
            .attachments
            .insert((request.key, request.provider_part_id), chunks);
        Ok(())
    }

    /// Makes checkpoints older than `sequence` fail with `InvalidCursor`.
    ///
    /// # Errors
    ///
    /// Returns a fixed fake-state error if the shared state is unavailable.
    pub fn invalidate_cursors_before(&self, sequence: u64) -> ProviderResult<()> {
        self.lock_state()?.invalid_before = Some(sequence);
        Ok(())
    }

    /// Duplicates the changes in the next successful sync page once.
    ///
    /// # Errors
    ///
    /// Returns a fixed fake-state error if the shared state is unavailable.
    pub fn duplicate_next_page(&self) -> ProviderResult<()> {
        self.lock_state()?.duplicate_next_page = true;
        Ok(())
    }

    /// Injects a one-shot typed failure for an operation.
    ///
    /// # Errors
    ///
    /// Returns a fixed fake-state error if the shared state is unavailable.
    pub fn inject_failure(
        &self,
        operation: FakeOperation,
        error: ProviderError,
    ) -> ProviderResult<()> {
        self.lock_state()?
            .failures
            .entry(operation)
            .or_default()
            .push_back(error);
        Ok(())
    }

    /// Queues a terminal send outcome; outcomes are consumed in FIFO order.
    ///
    /// # Errors
    ///
    /// Returns a fixed fake-state error if the shared state is unavailable.
    pub fn queue_send_outcome(&self, outcome: SendOutcome) -> ProviderResult<()> {
        self.lock_state()?.send_outcomes.push_back(outcome);
        Ok(())
    }

    /// Returns only allowlisted call metadata.
    ///
    /// # Errors
    ///
    /// Returns a fixed fake-state error if the shared state is unavailable.
    pub fn calls(&self) -> ProviderResult<Vec<FakeCall>> {
        Ok(self.lock_state()?.calls.clone())
    }

    fn lock_state(&self) -> ProviderResult<MutexGuard<'_, FakeProviderState>> {
        self.state.lock().map_err(|_| internal_fake_error())
    }
}

impl MailProvider for FakeMailProvider {
    fn provider(&self) -> Provider {
        self.provider
    }

    fn initial_sync<'a>(
        &'a self,
        request: InitialSyncRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, SyncPage> {
        Box::pin(async move {
            ensure_not_cancelled(cancellation)?;
            let mut state = self.lock_state()?;
            state.calls.push(FakeCall::InitialSync {
                limit: request.limit.get(),
                continued: request.continuation.is_some(),
            });
            take_failure(&mut state, FakeOperation::InitialSync)?;

            let (offset, snapshot, original_limit) = match request.continuation.as_ref() {
                Some(continuation) => parse_triplet(continuation.cursor())?,
                None => (0, current_sequence(&state), u64::from(request.limit.get())),
            };
            if original_limit != u64::from(request.limit.get()) {
                return Err(ProviderError::new(
                    ProviderErrorKind::Protocol,
                    "fake_initial_limit_changed",
                ));
            }

            build_page(
                &mut state,
                0,
                offset,
                snapshot,
                original_limit,
                original_limit,
            )
        })
    }

    fn incremental_sync<'a>(
        &'a self,
        request: IncrementalSyncRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, SyncPage> {
        Box::pin(async move {
            ensure_not_cancelled(cancellation)?;
            let mut state = self.lock_state()?;
            state.calls.push(FakeCall::IncrementalSync {
                continued: request.continuation.is_some(),
            });
            take_failure(&mut state, FakeOperation::IncrementalSync)?;

            let base = parse_checkpoint(&request.cursor)?;
            if state.invalid_before.is_some_and(|minimum| base < minimum) {
                return Err(ProviderError::new(
                    ProviderErrorKind::InvalidCursor,
                    "fake_cursor_invalidated",
                ));
            }

            let (offset, snapshot, continued_base) = match request.continuation.as_ref() {
                Some(continuation) => parse_triplet(continuation.cursor())?,
                None => (0, current_sequence(&state), base),
            };
            if continued_base != base {
                return Err(ProviderError::new(
                    ProviderErrorKind::Protocol,
                    "fake_incremental_cursor_changed",
                ));
            }

            build_page(&mut state, base, offset, snapshot, u64::MAX, base)
        })
    }

    fn fetch_body<'a>(
        &'a self,
        request: FetchBodyRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, NormalizedMimeMessage> {
        Box::pin(async move {
            ensure_not_cancelled(cancellation)?;
            let mut state = self.lock_state()?;
            state.calls.push(FakeCall::FetchBody);
            take_failure(&mut state, FakeOperation::FetchBody)?;
            state
                .messages
                .get(&request.key)
                .map(|message| message.mime.clone())
                .ok_or_else(not_found_error)
        })
    }

    fn fetch_attachment<'a>(
        &'a self,
        request: AttachmentRequest,
        sink: &'a mut dyn AttachmentSink,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, AttachmentDownload> {
        Box::pin(async move {
            ensure_not_cancelled(cancellation)?;
            let chunks = {
                let mut state = self.lock_state()?;
                state.calls.push(FakeCall::FetchAttachment);
                take_failure(&mut state, FakeOperation::FetchAttachment)?;
                state
                    .attachments
                    .get(&(request.key, request.provider_part_id))
                    .cloned()
                    .ok_or_else(not_found_error)?
            };

            let mut bytes_written = 0_u64;
            for chunk in chunks {
                ensure_not_cancelled(cancellation)?;
                sink.write_chunk(&chunk).await.map_err(|_| {
                    ProviderError::new(ProviderErrorKind::Permanent, "attachment_sink_rejected")
                })?;
                bytes_written = bytes_written.saturating_add(chunk.len() as u64);
            }
            Ok(AttachmentDownload {
                bytes_written,
                checksum_sha256: None,
            })
        })
    }

    fn set_read<'a>(
        &'a self,
        request: SetReadRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, ReadStateAck> {
        Box::pin(async move {
            ensure_not_cancelled(cancellation)?;
            let mut state = self.lock_state()?;
            state.calls.push(FakeCall::SetRead {
                desired_read: request.desired_read,
            });
            take_failure(&mut state, FakeOperation::SetRead)?;

            let sequence = next_sequence(&state);
            let message = state
                .messages
                .get_mut(&request.key)
                .ok_or_else(not_found_error)?;
            if message.read == request.desired_read {
                return Ok(ReadStateAck {
                    key: request.key,
                    read: request.desired_read,
                    revision: message.provider_revision.clone(),
                });
            }
            message.read = request.desired_read;
            let revision = ProviderRevision::new(format!("fake-read-{sequence}"));
            message.provider_revision = Some(revision.clone());
            let change = RemoteChange::ReadState {
                key: request.key.clone(),
                read: request.desired_read,
                revision: Some(revision.clone()),
            };
            state.changes.push(SequencedChange { sequence, change });

            Ok(ReadStateAck {
                key: request.key,
                read: request.desired_read,
                revision: Some(revision),
            })
        })
    }

    fn send<'a>(
        &'a self,
        request: SendRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, SendOutcome> {
        Box::pin(async move {
            ensure_not_cancelled(cancellation)?;
            let mut state = self.lock_state()?;
            state.calls.push(FakeCall::Send {
                byte_count: request.message.as_bytes().len(),
                recipient_count: request.message.envelope.recipients.len(),
                provider_thread_present: request.provider_thread_id.is_some(),
            });
            take_failure(&mut state, FakeOperation::Send)?;

            Ok(state.send_outcomes.pop_front().unwrap_or_else(|| {
                SendOutcome::Accepted(AcceptedSend {
                    provider_message_id: None,
                    reconciliation_key: ReconciliationKey::new(request.message.message_id),
                })
            }))
        })
    }
}

/// Stateful authenticator fake that never stores callback URLs or authorization codes.
#[derive(Clone)]
pub struct FakeAuthenticator {
    account: AuthenticatedAccount,
    state: Arc<Mutex<FakeAuthenticatorState>>,
}

#[derive(Default)]
struct FakeAuthenticatorState {
    next_flow: u64,
    failures: HashMap<FakeOperation, VecDeque<ProviderError>>,
    revoked: HashSet<CredentialRef>,
    calls: Vec<FakeCall>,
}

impl FakeAuthenticator {
    #[must_use]
    pub fn new(account: AuthenticatedAccount) -> Self {
        Self {
            account,
            state: Arc::new(Mutex::new(FakeAuthenticatorState::default())),
        }
    }

    /// Injects a one-shot typed authentication failure.
    ///
    /// # Errors
    ///
    /// Returns a fixed fake-state error if the shared state is unavailable.
    pub fn inject_failure(
        &self,
        operation: FakeOperation,
        error: ProviderError,
    ) -> ProviderResult<()> {
        self.lock_state()?
            .failures
            .entry(operation)
            .or_default()
            .push_back(error);
        Ok(())
    }

    /// Returns only allowlisted authentication call metadata.
    ///
    /// # Errors
    ///
    /// Returns a fixed fake-state error if the shared state is unavailable.
    pub fn calls(&self) -> ProviderResult<Vec<FakeCall>> {
        Ok(self.lock_state()?.calls.clone())
    }

    fn lock_state(&self) -> ProviderResult<MutexGuard<'_, FakeAuthenticatorState>> {
        self.state.lock().map_err(|_| internal_fake_error())
    }
}

impl AccountAuthenticator for FakeAuthenticator {
    fn start_login<'a>(
        &'a self,
        request: StartLoginRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, LoginStart> {
        Box::pin(async move {
            ensure_not_cancelled(cancellation)?;
            let mut state = self.lock_state()?;
            state.calls.push(FakeCall::StartLogin {
                provider: request.provider,
            });
            take_auth_failure(&mut state, FakeOperation::StartLogin)?;
            state.next_flow = state.next_flow.saturating_add(1);
            Ok(LoginStart {
                flow_id: format!("fake-flow-{}", state.next_flow),
                authorization_url: SensitiveString::new("https://auth.example/fake"),
            })
        })
    }

    fn complete_login<'a>(
        &'a self,
        _request: CompleteLoginRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, AuthenticatedAccount> {
        Box::pin(async move {
            ensure_not_cancelled(cancellation)?;
            let mut state = self.lock_state()?;
            state.calls.push(FakeCall::CompleteLogin);
            take_auth_failure(&mut state, FakeOperation::CompleteLogin)?;
            Ok(self.account.clone())
        })
    }

    fn connect_with_authorization_code<'a>(
        &'a self,
        request: AuthorizationCodeLoginRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, AuthenticatedAccount> {
        Box::pin(async move {
            ensure_not_cancelled(cancellation)?;
            let mut state = self.lock_state()?;
            state.calls.push(FakeCall::AuthorizationCodeLogin {
                provider: request.provider,
            });
            take_auth_failure(&mut state, FakeOperation::AuthorizationCodeLogin)?;
            Ok(self.account.clone())
        })
    }

    fn refresh<'a>(
        &'a self,
        credential_ref: &'a CredentialRef,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, AuthenticatedAccount> {
        Box::pin(async move {
            ensure_not_cancelled(cancellation)?;
            let mut state = self.lock_state()?;
            state.calls.push(FakeCall::Refresh);
            take_auth_failure(&mut state, FakeOperation::Refresh)?;
            if state.revoked.contains(credential_ref) {
                return Err(ProviderError::new(
                    ProviderErrorKind::Authentication,
                    "fake_credential_revoked",
                ));
            }
            Ok(self.account.clone())
        })
    }

    fn revoke<'a>(
        &'a self,
        credential_ref: &'a CredentialRef,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, ()> {
        Box::pin(async move {
            ensure_not_cancelled(cancellation)?;
            let mut state = self.lock_state()?;
            state.calls.push(FakeCall::Revoke);
            take_auth_failure(&mut state, FakeOperation::Revoke)?;
            state.revoked.insert(credential_ref.clone());
            Ok(())
        })
    }
}

fn build_page(
    state: &mut FakeProviderState,
    base: u64,
    offset: u64,
    snapshot: u64,
    maximum: u64,
    continuation_anchor: u64,
) -> ProviderResult<SyncPage> {
    let eligible: Vec<RemoteChange> = state
        .changes
        .iter()
        .filter(|entry| entry.sequence > base && entry.sequence <= snapshot)
        .map(|entry| entry.change.clone())
        .take(usize::try_from(maximum).unwrap_or(usize::MAX))
        .collect();
    let start = usize::try_from(offset).map_err(|_| cursor_protocol_error())?;
    if start > eligible.len() {
        return Err(cursor_protocol_error());
    }
    let end = start.saturating_add(state.page_size).min(eligible.len());
    let mut changes = eligible[start..end].to_vec();
    if state.duplicate_next_page {
        if changes.len() > 1 {
            let duplicate = changes[0].clone();
            if let Some(last) = changes.last_mut() {
                *last = duplicate;
            }
        }
        state.duplicate_next_page = false;
    }

    let state_value = if end < eligible.len() {
        SyncPageState::More(PageContinuation::new(cursor_triplet(
            end as u64,
            snapshot,
            continuation_anchor,
        )?))
    } else {
        SyncPageState::Complete(DurableCheckpoint::new(cursor_checkpoint(snapshot)?))
    };

    Ok(SyncPage {
        mailboxes: state.mailboxes.clone(),
        changes,
        state: state_value,
    })
}

fn apply_change_to_messages(
    messages: &mut HashMap<RemoteMessageKey, RemoteMessage>,
    change: &RemoteChange,
) {
    match change {
        RemoteChange::Upsert(message) => {
            messages.insert(message.key.clone(), (**message).clone());
        }
        RemoteChange::ReadState {
            key,
            read,
            revision,
        } => {
            if let Some(message) = messages.get_mut(key) {
                message.read = *read;
                message.provider_revision.clone_from(revision);
            }
        }
        RemoteChange::Gone(key) => {
            messages.remove(key);
        }
    }
}

fn next_sequence(state: &FakeProviderState) -> u64 {
    current_sequence(state).saturating_add(1)
}

fn current_sequence(state: &FakeProviderState) -> u64 {
    state.changes.last().map_or(0, |entry| entry.sequence)
}

fn cursor_checkpoint(sequence: u64) -> ProviderResult<OpaqueProviderCursor> {
    OpaqueProviderCursor::from_json(sequence.to_string())
}

fn cursor_triplet(first: u64, second: u64, third: u64) -> ProviderResult<OpaqueProviderCursor> {
    OpaqueProviderCursor::from_json(format!("[{first},{second},{third}]"))
}

fn parse_checkpoint(checkpoint: &DurableCheckpoint) -> ProviderResult<u64> {
    checkpoint
        .cursor()
        .as_json()
        .parse::<u64>()
        .map_err(|_| cursor_protocol_error())
}

fn parse_triplet(cursor: &OpaqueProviderCursor) -> ProviderResult<(u64, u64, u64)> {
    let value = cursor.as_json();
    let inner = value
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .ok_or_else(cursor_protocol_error)?;
    let values = inner
        .split(',')
        .map(str::trim)
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| cursor_protocol_error())?;
    match values.as_slice() {
        [first, second, third] => Ok((*first, *second, *third)),
        _ => Err(cursor_protocol_error()),
    }
}

fn take_failure(state: &mut FakeProviderState, operation: FakeOperation) -> ProviderResult<()> {
    if let Some(error) = state
        .failures
        .get_mut(&operation)
        .and_then(VecDeque::pop_front)
    {
        Err(error)
    } else {
        Ok(())
    }
}

fn take_auth_failure(
    state: &mut FakeAuthenticatorState,
    operation: FakeOperation,
) -> ProviderResult<()> {
    if let Some(error) = state
        .failures
        .get_mut(&operation)
        .and_then(VecDeque::pop_front)
    {
        Err(error)
    } else {
        Ok(())
    }
}

fn ensure_not_cancelled(cancellation: &dyn Cancellation) -> ProviderResult<()> {
    if cancellation.is_cancelled() {
        Err(ProviderError::new(
            ProviderErrorKind::Cancelled,
            "operation_cancelled",
        ))
    } else {
        Ok(())
    }
}

const fn cursor_protocol_error() -> ProviderError {
    ProviderError::new(ProviderErrorKind::Protocol, "fake_cursor_invalid")
}

const fn not_found_error() -> ProviderError {
    ProviderError::new(ProviderErrorKind::Permanent, "fake_remote_item_not_found")
}

const fn internal_fake_error() -> ProviderError {
    ProviderError::new(ProviderErrorKind::Permanent, "fake_state_unavailable")
}
