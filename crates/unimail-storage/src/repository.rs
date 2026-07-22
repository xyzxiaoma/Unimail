use std::{
    fs,
    path::{Component, Path, PathBuf},
    str::FromStr,
    sync::Arc,
};

use rusqlite::{Connection, OptionalExtension, params};
use unicode_normalization::UnicodeNormalization;
use unimail_core::{
    Account, AccountAuthState, AccountAuthUpdateInput, AccountConnectInput, AccountConnectResult,
    AccountCreateInput, AccountId, AddressRole, Attachment, AttachmentDownloadSource, AttachmentId,
    AttachmentVerificationInput, AuthorizeOutboundRetryInput, ClaimDesiredReadMutationInput,
    ClaimSyncOperationInput, CompleteDesiredReadMutationInput, CompleteOutboundAttemptInput,
    ComposedMessage, CredentialRef, CredentialStore, DeleteAccountResult, DeliveryEnvelope,
    DesiredReadMutation, DesiredReadMutationState, Draft, DraftAddress, DraftAttachmentInput,
    DraftId, DraftSaveInput, DraftSendReview, DraftSendReviewKey, DraftSendReviewReason,
    DraftSummary, DurableCheckpoint, InboxListInput, InitialSyncLimit, LeaseRecoveryResult,
    Mailbox, MailboxId, MailboxRole, MailboxUpsertInput, MessageAddress, MessageAddressInput,
    MessageDetail, MessageDirection, MessageId, MessageListInput, MessagePage, MessagePageCursor,
    MessageReadStateInput, MessageSummary, MessageUpsertInput, MessageUpsertResult,
    MimeAddressRole, OfflineDraftReviewInput, OfflineDraftReviewResult, OpaqueProviderCursor,
    OperationId, OperationLease, OutboundAttempt, OutboundAttemptId, OutboundAttemptOutcome,
    OutboundAttemptSnapshot, OutboundAttemptState, OutboundFailureCode,
    PrepareOutboundAttemptInput, Provider, ProviderRevision, ReadIntentGeneration,
    ReconcileOutboundAttemptInput, RecordSentRefreshInput, RemoteChange, RemoteMailbox,
    RemoteMessage, RemoteMessageKey, ReplySource, RepositoryError, RepositoryResult, SafeErrorCode,
    ScheduleSyncInput, SearchMessageCursor, SearchMessageHit, SearchMessagePage,
    SearchMessagesInput, SendConfirmationRequired, SentProjection, StorageRepository,
    StorageStatus, SyncBatchInput, SyncBatchResult, SyncCursor, SyncCursorKey, SyncMode,
    SyncOperation, SyncOperationSummary, SyncStage, SyncState, SyncTrigger, SyncTriggerSet,
    TransitionDesiredReadMutationInput, TransitionSyncOperationInput,
};

use crate::{
    ConnectionFactory, EncryptedStore, NativeCredentialStore, StorageError,
    permissions::{configure_private_file_creation, ensure_private_directory},
};

const SEARCH_DOCUMENT_VERSION: u32 = 1;
const MAX_SEARCH_TERMS: usize = 48;
const MAX_SEARCH_CONTEXT_CHARS: usize = 180;
const ATTACHMENT_PARTIAL_PREFIX: &str = ".unimail-part-";

/// Backend-only file transfer created and tracked by encrypted storage.
pub struct AttachmentTransfer {
    operation_id: OperationId,
    file: Option<fs::File>,
    temporary_path: PathBuf,
    destination_path: PathBuf,
}

impl AttachmentTransfer {
    /// Moves the newly created file handle into an asynchronous attachment sink.
    ///
    /// # Errors
    ///
    /// Returns a safe repository category if the handle was already consumed.
    pub fn take_file(&mut self) -> RepositoryResult<fs::File> {
        self.file.take().ok_or(RepositoryError::ConstraintViolation)
    }

    /// Returns the backend-only operation identifier.
    #[must_use]
    pub const fn operation_id(&self) -> OperationId {
        self.operation_id
    }
}

/// `SQLCipher` implementation of Unimail's synchronous repository port.
pub struct SqlCipherRepository {
    store: EncryptedStore,
    credentials: Arc<dyn CredentialStore>,
    attachment_cache_root: PathBuf,
}

impl SqlCipherRepository {
    /// Initializes storage with the platform-native Credential Manager or Keychain.
    ///
    /// # Errors
    ///
    /// Returns a safe repository category if initialization or recovery cannot complete.
    pub fn initialize_with_native(
        database_path: impl Into<PathBuf>,
        service_name: impl Into<String>,
    ) -> RepositoryResult<Self> {
        let credentials: Arc<dyn CredentialStore> =
            Arc::new(NativeCredentialStore::new(service_name));
        Self::initialize(database_path, credentials)
    }

    /// Initializes storage with an injected credential-store port.
    ///
    /// # Errors
    ///
    /// Returns a safe repository category if initialization or recovery cannot complete.
    pub fn initialize(
        database_path: impl Into<PathBuf>,
        credentials: Arc<dyn CredentialStore>,
    ) -> RepositoryResult<Self> {
        let database_path = database_path.into();
        let attachment_cache_root = database_path.parent().map_or_else(
            || PathBuf::from("attachments"),
            |parent| parent.join("attachments"),
        );
        ensure_private_directory(&attachment_cache_root)
            .map_err(|_| RepositoryError::DatabaseOpenFailed)?;
        let cache_metadata = fs::symlink_metadata(&attachment_cache_root)
            .map_err(|_| RepositoryError::DatabaseOpenFailed)?;
        if cache_metadata.file_type().is_symlink() || !cache_metadata.is_dir() {
            return Err(RepositoryError::DatabaseOpenFailed);
        }
        let factory = ConnectionFactory::credentials(&database_path, Arc::clone(&credentials));
        let store = EncryptedStore::initialize(&factory).map_err(map_storage_error)?;
        let repository = Self {
            store,
            credentials,
            attachment_cache_root,
        };
        repository.resume_pending_cleanups()?;
        repository.drain_attachment_cleanup()?;
        repository.resume_attachment_transfers()?;
        repository.ensure_search_index()?;
        Ok(repository)
    }

    /// Returns only safe, non-sensitive readiness metadata.
    ///
    /// # Errors
    ///
    /// Reserved for repository health failures without exposing internal diagnostics.
    pub fn health(&self) -> RepositoryResult<StorageStatus> {
        let capabilities = self.store.capabilities();
        Ok(StorageStatus {
            ready: true,
            schema_version: capabilities.schema_version,
            cipher_available: !capabilities.cipher_version.is_empty(),
            fts5_available: capabilities.fts5_available,
            credential_store: capabilities.credential_store,
        })
    }

    /// Searches the FTS projection and returns matching message IDs in rank order.
    ///
    /// # Errors
    ///
    /// Returns a safe repository category if the query or stored IDs are invalid.
    pub fn search_message_ids(&self, query: &str, limit: u32) -> RepositoryResult<Vec<MessageId>> {
        let query = build_fts_query(query).map_err(map_storage_error)?;
        self.store
            .with_connection(|connection| {
                let mut statement = connection
                    .prepare(
                        "SELECT m.id FROM email_fts
                         JOIN messages m ON m.row_id = email_fts.message_row_id
                         WHERE email_fts MATCH ?1 ORDER BY rank LIMIT ?2",
                    )
                    .map_err(|error| StorageError::from_sql(&error))?;
                let rows = statement
                    .query_map(params![query, i64::from(limit.clamp(1, 100))], |row| {
                        row.get::<_, String>(0)
                    })
                    .map_err(|error| StorageError::from_sql(&error))?;
                rows.map(|row| {
                    let value = row.map_err(|error| StorageError::from_sql(&error))?;
                    MessageId::from_str(&value).map_err(|_| StorageError::Serialization)
                })
                .collect()
            })
            .map_err(map_storage_error)
    }

    fn ensure_search_index(&self) -> RepositoryResult<()> {
        let version = self
            .store
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT document_version FROM search_index_state WHERE singleton=1",
                        [],
                        |row| row.get::<_, u32>(0),
                    )
                    .map_err(|error| StorageError::from_sql(&error))
            })
            .map_err(map_storage_error)?;
        if version < SEARCH_DOCUMENT_VERSION {
            self.rebuild_search_index()?;
        }
        Ok(())
    }

    /// Creates one no-clobber transfer file and records it for restart cleanup.
    ///
    /// # Errors
    ///
    /// Returns a safe repository category when the destination is unsafe, occupied, or unwritable.
    pub fn begin_attachment_transfer(
        &self,
        operation_id: OperationId,
        destination_path: impl Into<PathBuf>,
        created_at_ms: i64,
    ) -> RepositoryResult<AttachmentTransfer> {
        let destination_path = destination_path.into();
        if created_at_ms < 0 || destination_path.file_name().is_none() {
            return Err(RepositoryError::ConstraintViolation);
        }
        match fs::symlink_metadata(&destination_path) {
            Ok(_) => return Err(RepositoryError::ConstraintViolation),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(RepositoryError::Internal),
        }
        let parent = destination_path
            .parent()
            .ok_or(RepositoryError::ConstraintViolation)?;
        let parent_metadata =
            fs::symlink_metadata(parent).map_err(|_| RepositoryError::ConstraintViolation)?;
        if parent_metadata.file_type().is_symlink() || !parent_metadata.is_dir() {
            return Err(RepositoryError::ConstraintViolation);
        }
        let temporary_path = parent.join(format!("{ATTACHMENT_PARTIAL_PREFIX}{operation_id}"));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        configure_private_file_creation(&mut options);
        let file = options
            .open(&temporary_path)
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::AlreadyExists => RepositoryError::ConstraintViolation,
                _ => RepositoryError::Internal,
            })?;
        let Some(temporary_path_text) = temporary_path.to_str() else {
            let _ = fs::remove_file(&temporary_path);
            return Err(RepositoryError::ConstraintViolation);
        };
        let recorded = self.store.with_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO attachment_transfer_cleanup(operation_id, temporary_path, created_at_ms)
                     VALUES (?1, ?2, ?3)",
                    params![operation_id.to_string(), temporary_path_text, created_at_ms],
                )
                .map_err(|error| StorageError::from_sql(&error))?;
            Ok(())
        });
        if let Err(error) = recorded {
            drop(file);
            let _ = fs::remove_file(&temporary_path);
            return Err(map_storage_error(error));
        }
        Ok(AttachmentTransfer {
            operation_id,
            file: Some(file),
            temporary_path,
            destination_path,
        })
    }

    /// Deletes one incomplete transfer and clears its encrypted cleanup record.
    ///
    /// # Errors
    ///
    /// Returns a safe cleanup category when deletion cannot be completed.
    pub fn abort_attachment_transfer(&self, transfer: &AttachmentTransfer) -> RepositoryResult<()> {
        remove_owned_transfer_file(&transfer.temporary_path)?;
        self.clear_attachment_transfer(transfer.operation_id)
    }

    /// Publishes a fully flushed transfer without overwriting an existing destination.
    ///
    /// # Errors
    ///
    /// Returns a safe constraint category for collisions or cleanup category for incomplete cleanup.
    pub fn finish_attachment_transfer(
        &self,
        transfer: &AttachmentTransfer,
    ) -> RepositoryResult<()> {
        if transfer.file.is_some() {
            return Err(RepositoryError::ConstraintViolation);
        }
        fs::hard_link(&transfer.temporary_path, &transfer.destination_path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                RepositoryError::ConstraintViolation
            } else {
                RepositoryError::Internal
            }
        })?;
        remove_owned_transfer_file(&transfer.temporary_path)?;
        self.clear_attachment_transfer(transfer.operation_id)
    }

    fn clear_attachment_transfer(&self, operation_id: OperationId) -> RepositoryResult<()> {
        self.store
            .with_connection(|connection| {
                connection
                    .execute(
                        "DELETE FROM attachment_transfer_cleanup WHERE operation_id=?1",
                        [operation_id.to_string()],
                    )
                    .map_err(|error| StorageError::from_sql(&error))?;
                Ok(())
            })
            .map_err(map_storage_error)
    }

    fn resume_attachment_transfers(&self) -> RepositoryResult<()> {
        let entries = self
            .store
            .with_connection(|connection| {
                let mut statement = connection
                    .prepare(
                        "SELECT operation_id, temporary_path
                         FROM attachment_transfer_cleanup ORDER BY created_at_ms, operation_id",
                    )
                    .map_err(|error| StorageError::from_sql(&error))?;
                statement
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(|error| StorageError::from_sql(&error))?
                    .map(|row| row.map_err(|error| StorageError::from_sql(&error)))
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(map_storage_error)?;
        for (operation_id, path) in entries {
            let operation_id =
                OperationId::from_str(&operation_id).map_err(|_| RepositoryError::InvalidData)?;
            let path = PathBuf::from(path);
            if is_owned_transfer_path(&path) {
                remove_owned_transfer_file(&path)?;
            }
            self.clear_attachment_transfer(operation_id)?;
        }
        Ok(())
    }

    fn resume_pending_cleanups(&self) -> RepositoryResult<()> {
        let ids = self
            .store
            .with_connection(|connection| {
                let mut statement = connection
                    .prepare("SELECT account_id FROM account_cleanup ORDER BY account_id")
                    .map_err(|error| StorageError::from_sql(&error))?;
                statement
                    .query_map([], |row| row.get::<_, String>(0))
                    .map_err(|error| StorageError::from_sql(&error))?
                    .map(|row| {
                        let raw = row.map_err(|error| StorageError::from_sql(&error))?;
                        parse_id(&raw)
                    })
                    .collect::<Result<Vec<AccountId>, StorageError>>()
            })
            .map_err(map_storage_error)?;
        for account_id in ids {
            self.delete_account_local(account_id)?;
        }
        Ok(())
    }

    fn drain_attachment_cleanup(&self) -> RepositoryResult<()> {
        let entries = self
            .store
            .with_connection(|connection| {
                let mut statement = connection
                    .prepare("SELECT account_id, cache_key FROM attachment_cleanup_queue ORDER BY account_id, cache_key")
                    .map_err(|error| StorageError::from_sql(&error))?;
                statement
                    .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
                    .map_err(|error| StorageError::from_sql(&error))?
                    .map(|row| row.map_err(|error| StorageError::from_sql(&error)))
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(map_storage_error)?;
        for (account_id, cache_key) in entries {
            let live_reference = self
                .store
                .with_connection(|connection| {
                    connection
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM attachments WHERE cache_key=?1)",
                            [&cache_key],
                            |row| row.get::<_, bool>(0),
                        )
                        .map_err(|error| StorageError::from_sql(&error))
                })
                .map_err(map_storage_error)?;
            if !live_reference {
                self.remove_cache_entry(&cache_key)?;
            }
            self.store
                .with_connection(|connection| {
                    connection
                        .execute(
                            "DELETE FROM attachment_cleanup_queue WHERE account_id=?1 AND cache_key=?2",
                            params![account_id, cache_key],
                        )
                        .map_err(|error| StorageError::from_sql(&error))?;
                    Ok(())
                })
                .map_err(map_storage_error)?;
        }
        Ok(())
    }

    fn remove_cache_entry(&self, cache_key: &str) -> RepositoryResult<()> {
        validate_cache_key(cache_key).map_err(map_storage_error)?;
        let path = self.attachment_cache_root.join(cache_key);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() || metadata.is_file() => {
                fs::remove_file(path).map_err(|_| RepositoryError::CleanupPending)
            }
            Ok(metadata) if metadata.is_dir() => {
                fs::remove_dir(path).map_err(|_| RepositoryError::CleanupPending)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Ok(_) | Err(_) => Err(RepositoryError::CleanupPending),
        }
    }
}

impl StorageRepository for SqlCipherRepository {
    fn create_account(&self, input: AccountCreateInput) -> RepositoryResult<Account> {
        if input.credential_ref.as_str() == crate::credentials::DATABASE_KEY_REF {
            return Err(RepositoryError::ConstraintViolation);
        }
        let account = Account {
            id: input.id,
            provider: input.provider,
            email: input.email,
            display_name: input.display_name,
            credential_ref: input.credential_ref,
            auth_state: input.auth_state,
            enabled: input.enabled,
            deleting: false,
            created_at_ms: input.created_at_ms,
            updated_at_ms: input.created_at_ms,
            last_error_code: None,
        };
        self.store
            .with_connection(|connection| {
                connection
                    .execute(
                        "INSERT INTO accounts(
                            id, provider, email, display_name, credential_ref, auth_state,
                            enabled, deleting, cleanup_state, created_at_ms, updated_at_ms
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, 'none', ?8, ?8)",
                        params![
                            account.id.to_string(),
                            provider_to_str(account.provider),
                            account.email,
                            account.display_name,
                            account.credential_ref.as_str(),
                            auth_state_to_str(account.auth_state),
                            account.enabled,
                            account.created_at_ms,
                        ],
                    )
                    .map_err(|error| StorageError::from_sql(&error))?;
                Ok(())
            })
            .map_err(map_storage_error)?;
        Ok(account)
    }

    fn connect_account(
        &self,
        input: AccountConnectInput,
    ) -> RepositoryResult<AccountConnectResult> {
        if input.email.trim().is_empty()
            || input.credential_ref.as_str() == crate::credentials::DATABASE_KEY_REF
        {
            return Err(RepositoryError::ConstraintViolation);
        }
        self.store
            .with_transaction(|transaction| connect_account(transaction, input))
            .map_err(map_storage_error)
    }

    fn update_account_auth(&self, input: AccountAuthUpdateInput) -> RepositoryResult<Account> {
        self.store
            .with_transaction(|transaction| update_account_auth(transaction, input))
            .map_err(map_storage_error)
    }

    fn list_accounts(&self) -> RepositoryResult<Vec<Account>> {
        self.store
            .with_connection(|connection| {
                let mut statement = connection
                    .prepare(
                        "SELECT id, provider, email, display_name, credential_ref, auth_state,
                                enabled, deleting, created_at_ms, updated_at_ms, last_error_code
                         FROM accounts WHERE deleting = 0 ORDER BY created_at_ms, id",
                    )
                    .map_err(|error| StorageError::from_sql(&error))?;
                let rows = statement
                    .query_map([], read_account_row)
                    .map_err(|error| StorageError::from_sql(&error))?;
                rows.map(|row| {
                    row.map_err(|error| StorageError::from_sql(&error))
                        .and_then(account_from_row)
                })
                .collect()
            })
            .map_err(map_storage_error)
    }

    fn get_account(&self, account_id: AccountId) -> RepositoryResult<Option<Account>> {
        self.store
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT id, provider, email, display_name, credential_ref, auth_state,
                                enabled, deleting, created_at_ms, updated_at_ms, last_error_code
                         FROM accounts WHERE id = ?1 AND deleting = 0",
                        [account_id.to_string()],
                        read_account_row,
                    )
                    .optional()
                    .map_err(|error| StorageError::from_sql(&error))?
                    .map(account_from_row)
                    .transpose()
            })
            .map_err(map_storage_error)
    }

    #[allow(clippy::too_many_lines)]
    fn delete_account_local(&self, account_id: AccountId) -> RepositoryResult<DeleteAccountResult> {
        let existing_cleanup = self
            .store
            .with_connection(|connection| load_cleanup(connection, account_id))
            .map_err(map_storage_error)?;
        let (credential_ref, cache_keys, mut stage) = if let Some(cleanup) = existing_cleanup {
            cleanup
        } else {
            self.store
                .with_transaction(|transaction| {
                    let credential_ref: Option<String> = transaction
                        .query_row(
                            "SELECT credential_ref FROM accounts WHERE id = ?1",
                            [account_id.to_string()],
                            |row| row.get(0),
                        )
                        .optional()
                        .map_err(|error| StorageError::from_sql(&error))?;
                    let Some(credential_ref) = credential_ref else {
                        return Ok((String::new(), Vec::new(), String::new()));
                    };
                    let cache_keys = load_account_cache_keys(transaction, account_id)?;
                    let encoded = serde_json::to_string(&cache_keys)
                        .map_err(|_| StorageError::Serialization)?;
                    transaction
                        .execute(
                            "UPDATE accounts SET deleting = 1, cleanup_state = 'credentials' WHERE id = ?1",
                            [account_id.to_string()],
                        )
                        .map_err(|error| StorageError::from_sql(&error))?;
                    transaction
                        .execute(
                            "INSERT INTO account_cleanup(
                                account_id, credential_ref, attachment_cache_keys_json, stage, updated_at_ms
                             ) VALUES (?1, ?2, ?3, 'credentials',
                                (SELECT updated_at_ms FROM accounts WHERE id = ?1))",
                            params![account_id.to_string(), credential_ref, encoded],
                        )
                        .map_err(|error| StorageError::from_sql(&error))?;
                    Ok((credential_ref, cache_keys, "credentials".to_owned()))
                })
                .map_err(map_storage_error)?
        };

        if stage.is_empty() {
            return Ok(DeleteAccountResult {
                deleted: false,
                credential_refs: Vec::new(),
                attachment_cache_keys: Vec::new(),
            });
        }

        if stage == "credentials" {
            self.credentials
                .delete(&CredentialRef::new(&credential_ref))
                .map_err(|_| RepositoryError::CleanupPending)?;
            self.store
                .with_connection(|connection| {
                    connection
                        .execute(
                            "UPDATE account_cleanup SET stage = 'database' WHERE account_id = ?1",
                            [account_id.to_string()],
                        )
                        .map_err(|error| StorageError::from_sql(&error))?;
                    Ok(())
                })
                .map_err(map_storage_error)?;
            "database".clone_into(&mut stage);
        }

        if stage == "database" {
            self.store
                .with_transaction(|transaction| {
                    transaction
                        .execute(
                            "DELETE FROM email_fts WHERE message_row_id IN
                             (SELECT row_id FROM messages WHERE account_id = ?1)",
                            [account_id.to_string()],
                        )
                        .map_err(|error| StorageError::from_sql(&error))?;
                    transaction
                        .execute("DELETE FROM accounts WHERE id = ?1", [account_id.to_string()])
                        .map_err(|error| StorageError::from_sql(&error))?;
                    transaction
                        .execute(
                            "UPDATE account_cleanup SET stage = 'attachments' WHERE account_id = ?1",
                            [account_id.to_string()],
                        )
                        .map_err(|error| StorageError::from_sql(&error))?;
                    Ok(())
                })
                .map_err(map_storage_error)?;
            "attachments".clone_into(&mut stage);
        }

        if stage == "attachments" {
            for cache_key in &cache_keys {
                self.remove_cache_entry(cache_key)?;
            }
            self.store
                .with_transaction(|transaction| {
                    transaction
                        .execute(
                            "DELETE FROM attachment_cleanup_queue WHERE account_id=?1",
                            [account_id.to_string()],
                        )
                        .map_err(|error| StorageError::from_sql(&error))?;
                    transaction
                        .execute(
                            "DELETE FROM account_cleanup WHERE account_id=?1",
                            [account_id.to_string()],
                        )
                        .map_err(|error| StorageError::from_sql(&error))?;
                    Ok(())
                })
                .map_err(map_storage_error)?;
        }

        Ok(DeleteAccountResult {
            deleted: true,
            credential_refs: vec![CredentialRef::new(credential_ref)],
            attachment_cache_keys: cache_keys,
        })
    }

    fn upsert_mailbox(&self, input: MailboxUpsertInput) -> RepositoryResult<Mailbox> {
        self.store
            .with_transaction(|transaction| upsert_mailbox(transaction, &input))
            .map_err(map_storage_error)
    }

    fn upsert_message(&self, input: MessageUpsertInput) -> RepositoryResult<MessageUpsertResult> {
        self.store
            .with_transaction(|transaction| upsert_message(transaction, &input))
            .map_err(map_storage_error)
    }

    fn list_messages(&self, input: &MessageListInput) -> RepositoryResult<MessagePage> {
        self.store
            .with_connection(|connection| list_messages(connection, input))
            .map_err(map_storage_error)
    }

    fn list_inbox_messages(&self, input: &InboxListInput) -> RepositoryResult<MessagePage> {
        self.store
            .with_connection(|connection| list_inbox_messages(connection, input))
            .map_err(map_storage_error)
    }

    fn search_inbox_messages(
        &self,
        input: &SearchMessagesInput,
    ) -> RepositoryResult<SearchMessagePage> {
        self.store
            .with_connection(|connection| search_inbox_messages(connection, input))
            .map_err(map_storage_error)
    }

    fn rebuild_search_index(&self) -> RepositoryResult<()> {
        self.store
            .with_transaction(|transaction| rebuild_search_index(transaction))
            .map_err(map_storage_error)
    }

    fn get_attachment_download_source(
        &self,
        attachment_id: AttachmentId,
    ) -> RepositoryResult<Option<AttachmentDownloadSource>> {
        self.store
            .with_connection(|connection| get_attachment_download_source(connection, attachment_id))
            .map_err(map_storage_error)
    }

    fn record_attachment_verification(
        &self,
        input: AttachmentVerificationInput,
    ) -> RepositoryResult<()> {
        self.store
            .with_transaction(|transaction| record_attachment_verification(transaction, &input))
            .map_err(map_storage_error)
    }

    fn get_message(&self, message_id: MessageId) -> RepositoryResult<Option<MessageDetail>> {
        self.store
            .with_connection(|connection| get_message(connection, message_id))
            .map_err(map_storage_error)
    }

    fn get_reply_source(&self, message_id: MessageId) -> RepositoryResult<Option<ReplySource>> {
        self.store
            .with_connection(|connection| get_reply_source(connection, message_id))
            .map_err(map_storage_error)
    }

    fn set_message_read(
        &self,
        input: MessageReadStateInput,
    ) -> RepositoryResult<DesiredReadMutation> {
        self.store
            .with_transaction(|transaction| set_message_read(transaction, input))
            .map_err(map_storage_error)
    }

    fn list_due_desired_read_mutations(
        &self,
        account_id: AccountId,
        now_ms: i64,
        limit: u32,
    ) -> RepositoryResult<Vec<DesiredReadMutation>> {
        self.store
            .with_connection(|connection| {
                list_due_desired_read_mutations(connection, account_id, now_ms, limit)
            })
            .map_err(map_storage_error)
    }

    fn claim_desired_read_mutation(
        &self,
        input: ClaimDesiredReadMutationInput,
    ) -> RepositoryResult<Option<DesiredReadMutation>> {
        self.store
            .with_transaction(|transaction| claim_desired_read_mutation(transaction, &input))
            .map_err(map_storage_error)
    }

    fn complete_desired_read_mutation(
        &self,
        input: CompleteDesiredReadMutationInput,
    ) -> RepositoryResult<bool> {
        self.store
            .with_transaction(|transaction| complete_desired_read_mutation(transaction, &input))
            .map_err(map_storage_error)
    }

    fn transition_desired_read_mutation(
        &self,
        input: TransitionDesiredReadMutationInput,
    ) -> RepositoryResult<bool> {
        self.store
            .with_connection(|connection| transition_desired_read_mutation(connection, &input))
            .map_err(map_storage_error)
    }

    fn save_draft(&self, input: DraftSaveInput) -> RepositoryResult<Draft> {
        self.store
            .with_transaction(|transaction| save_draft(transaction, &input))
            .map_err(map_storage_error)
    }

    fn get_draft(&self, draft_id: DraftId) -> RepositoryResult<Option<Draft>> {
        self.store
            .with_connection(|connection| get_draft(connection, draft_id))
            .map_err(map_storage_error)
    }

    fn list_drafts(&self, account_id: Option<AccountId>) -> RepositoryResult<Vec<DraftSummary>> {
        self.store
            .with_connection(|connection| list_drafts(connection, account_id))
            .map_err(map_storage_error)
    }

    fn delete_draft(&self, draft_id: DraftId) -> RepositoryResult<bool> {
        self.store
            .with_connection(|connection| {
                connection
                    .execute("DELETE FROM drafts WHERE id = ?1", [draft_id.to_string()])
                    .map(|changed| changed > 0)
                    .map_err(|error| StorageError::from_sql(&error))
            })
            .map_err(map_storage_error)
    }

    fn save_draft_for_offline_review(
        &self,
        input: OfflineDraftReviewInput,
    ) -> RepositoryResult<OfflineDraftReviewResult> {
        self.store
            .with_transaction(|transaction| save_draft_for_offline_review(transaction, &input))
            .map_err(map_storage_error)
    }

    fn list_send_confirmation_required(
        &self,
        account_id: Option<AccountId>,
    ) -> RepositoryResult<Vec<SendConfirmationRequired>> {
        self.store
            .with_connection(|connection| list_send_confirmation_required(connection, account_id))
            .map_err(map_storage_error)
    }

    fn consume_draft_send_review(&self, key: DraftSendReviewKey) -> RepositoryResult<bool> {
        self.store
            .with_connection(|connection| consume_draft_send_review(connection, key))
            .map_err(map_storage_error)
    }

    fn prepare_outbound_attempt(
        &self,
        input: PrepareOutboundAttemptInput,
    ) -> RepositoryResult<OutboundAttempt> {
        self.store
            .with_transaction(|transaction| prepare_outbound_attempt(transaction, &input))
            .map_err(map_storage_error)
    }

    fn complete_outbound_attempt(
        &self,
        input: CompleteOutboundAttemptInput,
    ) -> RepositoryResult<OutboundAttempt> {
        self.store
            .with_transaction(|transaction| complete_outbound_attempt(transaction, &input))
            .map_err(map_storage_error)
    }

    fn get_outbound_attempt(
        &self,
        attempt_id: OutboundAttemptId,
    ) -> RepositoryResult<Option<OutboundAttempt>> {
        self.store
            .with_connection(|connection| get_outbound_attempt(connection, attempt_id))
            .map_err(map_storage_error)
    }

    fn list_sent_projections(
        &self,
        account_id: Option<AccountId>,
    ) -> RepositoryResult<Vec<SentProjection>> {
        self.store
            .with_connection(|connection| list_sent_projections(connection, account_id))
            .map_err(map_storage_error)
    }

    fn record_sent_refresh(&self, input: RecordSentRefreshInput) -> RepositoryResult<u32> {
        self.store
            .with_connection(|connection| record_sent_refresh(connection, input))
            .map_err(map_storage_error)
    }

    fn authorize_outbound_retry(
        &self,
        input: AuthorizeOutboundRetryInput,
    ) -> RepositoryResult<bool> {
        self.store
            .with_connection(|connection| authorize_outbound_retry(connection, input))
            .map_err(map_storage_error)
    }

    fn reconcile_outbound_attempt(
        &self,
        input: ReconcileOutboundAttemptInput,
    ) -> RepositoryResult<OutboundAttempt> {
        self.store
            .with_transaction(|transaction| reconcile_outbound_attempt(transaction, &input))
            .map_err(map_storage_error)
    }

    fn recover_submitting_outbound_attempts(&self, recovered_at_ms: i64) -> RepositoryResult<u32> {
        self.store
            .with_connection(|connection| {
                recover_submitting_outbound_attempts(connection, recovered_at_ms)
            })
            .map_err(map_storage_error)
    }

    fn get_sync_cursor(&self, key: &SyncCursorKey) -> RepositoryResult<Option<SyncCursor>> {
        self.store
            .with_connection(|connection| get_sync_cursor(connection, key))
            .map_err(map_storage_error)
    }

    fn schedule_sync_operation(&self, input: ScheduleSyncInput) -> RepositoryResult<SyncOperation> {
        self.store
            .with_transaction(|transaction| schedule_sync_operation(transaction, &input))
            .map_err(map_storage_error)
    }

    fn list_runnable_sync_operations(
        &self,
        provider: Provider,
        now_ms: i64,
        limit: u32,
    ) -> RepositoryResult<Vec<SyncOperationSummary>> {
        self.store
            .with_connection(|connection| {
                list_runnable_sync_operations(connection, provider, now_ms, limit)
            })
            .map_err(map_storage_error)
    }

    fn claim_sync_operation(
        &self,
        input: ClaimSyncOperationInput,
    ) -> RepositoryResult<Option<SyncOperation>> {
        self.store
            .with_transaction(|transaction| claim_sync_operation(transaction, input))
            .map_err(map_storage_error)
    }

    fn transition_sync_operation(
        &self,
        input: TransitionSyncOperationInput,
    ) -> RepositoryResult<bool> {
        self.store
            .with_connection(|connection| transition_sync_operation(connection, &input))
            .map_err(map_storage_error)
    }

    fn request_sync_cancellation(
        &self,
        operation_id: OperationId,
        requested_at_ms: i64,
    ) -> RepositoryResult<bool> {
        self.store
            .with_connection(|connection| {
                connection
                    .execute(
                        "UPDATE sync_operations
                         SET state='cancelled', stage=NULL,
                             cancel_generation=cancel_generation + 1,
                             lease_id=NULL, lease_expires_at_ms=NULL,
                             next_attempt_at_ms=NULL, safe_error_code=NULL,
                             updated_at_ms=max(updated_at_ms, created_at_ms, ?2),
                             finished_at_ms=max(updated_at_ms, created_at_ms, ?2)
                         WHERE id = ?1 AND state NOT IN ('committed', 'failed', 'cancelled')",
                        params![operation_id.to_string(), requested_at_ms],
                    )
                    .map(|changed| changed > 0)
                    .map_err(|error| StorageError::from_sql(&error))
            })
            .map_err(map_storage_error)
    }

    fn mark_account_offline(
        &self,
        account_id: AccountId,
        updated_at_ms: i64,
    ) -> RepositoryResult<u32> {
        self.store
            .with_connection(|connection| {
                connection
                    .execute(
                        "UPDATE sync_operations
                         SET state='offline', stage=NULL,
                             cancel_generation=cancel_generation + 1,
                             lease_id=NULL, lease_expires_at_ms=NULL,
                             next_attempt_at_ms=NULL,
                             updated_at_ms=max(updated_at_ms, created_at_ms, ?2)
                         WHERE account_id=?1
                           AND state IN ('scheduled', 'running', 'waiting_backoff')",
                        params![account_id.to_string(), updated_at_ms],
                    )
                    .map_err(|error| StorageError::from_sql(&error))
                    .and_then(|count| u32::try_from(count).map_err(|_| StorageError::Serialization))
            })
            .map_err(map_storage_error)
    }

    fn restore_account_connectivity(
        &self,
        account_id: AccountId,
        updated_at_ms: i64,
    ) -> RepositoryResult<u32> {
        self.store
            .with_connection(|connection| {
                connection
                    .execute(
                        "UPDATE sync_operations
                         SET state='scheduled',
                             trigger_bits=(trigger_bits | ?3),
                             next_attempt_at_ms=NULL,
                             updated_at_ms=max(updated_at_ms, created_at_ms, ?2)
                         WHERE account_id=?1 AND state='offline'",
                        params![
                            account_id.to_string(),
                            updated_at_ms,
                            SyncTrigger::ConnectivityRestored.bit()
                        ],
                    )
                    .map_err(|error| StorageError::from_sql(&error))
                    .and_then(|count| u32::try_from(count).map_err(|_| StorageError::Serialization))
            })
            .map_err(map_storage_error)
    }

    fn get_sync_operation(
        &self,
        operation_id: OperationId,
    ) -> RepositoryResult<Option<SyncOperationSummary>> {
        self.store
            .with_connection(|connection| get_sync_operation_summary(connection, operation_id))
            .map_err(map_storage_error)
    }

    fn list_sync_operations(
        &self,
        account_id: AccountId,
        limit: u32,
    ) -> RepositoryResult<Vec<SyncOperationSummary>> {
        self.store
            .with_connection(|connection| list_sync_operations(connection, account_id, limit))
            .map_err(map_storage_error)
    }

    fn recover_expired_leases(&self, now_ms: i64) -> RepositoryResult<LeaseRecoveryResult> {
        self.store
            .with_transaction(|transaction| recover_expired_leases(transaction, now_ms))
            .map_err(map_storage_error)
    }

    fn commit_sync_batch(&self, input: SyncBatchInput) -> RepositoryResult<SyncBatchResult> {
        self.store
            .with_transaction(|transaction| commit_sync_batch(transaction, &input))
            .map_err(map_storage_error)
    }

    fn health(&self) -> RepositoryResult<StorageStatus> {
        Self::health(self)
    }
}

type AccountRow = (
    String,
    String,
    String,
    Option<String>,
    String,
    String,
    bool,
    bool,
    i64,
    i64,
    Option<String>,
);

type DesiredReadSourceRow = (String, String, String, Option<String>, i64, i64, i64);

fn read_account_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AccountRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
    ))
}

fn account_from_row(row: AccountRow) -> Result<Account, StorageError> {
    Ok(Account {
        id: parse_id(&row.0)?,
        provider: provider_from_str(&row.1)?,
        email: row.2,
        display_name: row.3,
        credential_ref: CredentialRef::new(row.4),
        auth_state: auth_state_from_str(&row.5)?,
        enabled: row.6,
        deleting: row.7,
        created_at_ms: row.8,
        updated_at_ms: row.9,
        last_error_code: row.10,
    })
}

fn connect_account(
    transaction: &rusqlite::Transaction<'_>,
    input: AccountConnectInput,
) -> Result<AccountConnectResult, StorageError> {
    let existing = transaction
        .query_row(
            "SELECT id, provider, email, display_name, credential_ref, auth_state,
                    enabled, deleting, created_at_ms, updated_at_ms, last_error_code
             FROM accounts WHERE provider=?1 AND email=?2 AND deleting=0",
            params![provider_to_str(input.provider), &input.email],
            read_account_row,
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))?
        .map(account_from_row)
        .transpose()?;

    if let Some(existing) = existing {
        let updated_at_ms = input
            .connected_at_ms
            .max(existing.created_at_ms)
            .max(existing.updated_at_ms);
        transaction
            .execute(
                "UPDATE accounts
                 SET display_name=?2, credential_ref=?3, auth_state='connected', enabled=1,
                     last_error_code=NULL, updated_at_ms=?4
                 WHERE id=?1 AND deleting=0",
                params![
                    existing.id.to_string(),
                    input.display_name.as_deref(),
                    input.credential_ref.as_str(),
                    updated_at_ms,
                ],
            )
            .map_err(|error| StorageError::from_sql(&error))?;
        let replaced_credential_ref =
            (existing.credential_ref != input.credential_ref).then_some(existing.credential_ref);
        return Ok(AccountConnectResult {
            account: Account {
                id: existing.id,
                provider: existing.provider,
                email: existing.email,
                display_name: input.display_name,
                credential_ref: input.credential_ref,
                auth_state: AccountAuthState::Connected,
                enabled: true,
                deleting: false,
                created_at_ms: existing.created_at_ms,
                updated_at_ms,
                last_error_code: None,
            },
            replaced_credential_ref,
            created: false,
        });
    }

    let account = Account {
        id: input.id,
        provider: input.provider,
        email: input.email,
        display_name: input.display_name,
        credential_ref: input.credential_ref,
        auth_state: AccountAuthState::Connected,
        enabled: true,
        deleting: false,
        created_at_ms: input.connected_at_ms.max(0),
        updated_at_ms: input.connected_at_ms.max(0),
        last_error_code: None,
    };
    transaction
        .execute(
            "INSERT INTO accounts(
                id, provider, email, display_name, credential_ref, auth_state,
                enabled, deleting, cleanup_state, created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'connected', 1, 0, 'none', ?6, ?6)",
            params![
                account.id.to_string(),
                provider_to_str(account.provider),
                &account.email,
                account.display_name.as_deref(),
                account.credential_ref.as_str(),
                account.created_at_ms,
            ],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    Ok(AccountConnectResult {
        account,
        replaced_credential_ref: None,
        created: true,
    })
}

fn update_account_auth(
    transaction: &rusqlite::Transaction<'_>,
    input: AccountAuthUpdateInput,
) -> Result<Account, StorageError> {
    let existing = transaction
        .query_row(
            "SELECT id, provider, email, display_name, credential_ref, auth_state,
                    enabled, deleting, created_at_ms, updated_at_ms, last_error_code
             FROM accounts WHERE id=?1 AND deleting=0",
            [input.account_id.to_string()],
            read_account_row,
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))?
        .map(account_from_row)
        .transpose()?
        .ok_or(StorageError::NotFound)?;
    let updated_at_ms = input
        .updated_at_ms
        .max(existing.created_at_ms)
        .max(existing.updated_at_ms);
    transaction
        .execute(
            "UPDATE accounts SET auth_state=?2, last_error_code=?3, updated_at_ms=?4
             WHERE id=?1 AND deleting=0",
            params![
                input.account_id.to_string(),
                auth_state_to_str(input.auth_state),
                input
                    .safe_error_code
                    .as_ref()
                    .map(unimail_core::SafeErrorCode::as_str),
                updated_at_ms,
            ],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    Ok(Account {
        auth_state: input.auth_state,
        last_error_code: input.safe_error_code.map(|code| code.as_str().to_owned()),
        updated_at_ms,
        ..existing
    })
}

fn upsert_mailbox(
    connection: &Connection,
    input: &MailboxUpsertInput,
) -> Result<Mailbox, StorageError> {
    let existing: Option<(String, i64, i64)> = connection
        .query_row(
            "SELECT id, created_at_ms, updated_at_ms FROM mailboxes
             WHERE account_id = ?1 AND provider_mailbox_id = ?2",
            params![input.account_id.to_string(), input.provider_mailbox_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))?;
    let (id, created_at_ms, current_updated_at_ms) = match existing {
        Some((id, created, updated)) => (parse_id(&id)?, created, updated),
        None => (
            input.id,
            input.updated_at_ms.max(0),
            input.updated_at_ms.max(0),
        ),
    };
    let updated_at_ms =
        clamp_durable_time(input.updated_at_ms, created_at_ms, current_updated_at_ms);
    connection
        .execute(
            "INSERT INTO mailboxes(
                id, account_id, provider_mailbox_id, role, display_name,
                created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(account_id, provider_mailbox_id) DO UPDATE SET
                role = excluded.role, display_name = excluded.display_name,
                updated_at_ms = max(mailboxes.updated_at_ms, excluded.updated_at_ms)",
            params![
                id.to_string(),
                input.account_id.to_string(),
                input.provider_mailbox_id,
                mailbox_role_to_str(input.role),
                input.display_name,
                created_at_ms,
                updated_at_ms,
            ],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    Ok(Mailbox {
        id,
        account_id: input.account_id,
        provider_mailbox_id: input.provider_mailbox_id.clone(),
        role: input.role,
        display_name: input.display_name.clone(),
        created_at_ms,
        updated_at_ms,
    })
}

#[allow(clippy::too_many_lines)]
fn upsert_message(
    connection: &Connection,
    input: &MessageUpsertInput,
) -> Result<MessageUpsertResult, StorageError> {
    let mailbox_account: Option<String> = connection
        .query_row(
            "SELECT account_id FROM mailboxes WHERE id=?1",
            [input.mailbox_id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))?;
    let expected_account = input.account_id.to_string();
    if mailbox_account.as_deref() != Some(expected_account.as_str()) {
        return Err(StorageError::Constraint);
    }
    for cache_key in input
        .attachments
        .iter()
        .filter_map(|attachment| attachment.cache_key.as_deref())
    {
        validate_cache_key(cache_key)?;
        reject_account_cleanup_key(connection, cache_key)?;
        connection
            .execute(
                "DELETE FROM attachment_cleanup_queue WHERE cache_key=?1",
                [cache_key],
            )
            .map_err(|error| StorageError::from_sql(&error))?;
    }
    let existing: Option<(String, i64, i64)> = connection
        .query_row(
            "SELECT id, row_id, created_at_ms FROM messages
             WHERE account_id = ?1 AND provider_message_id = ?2",
            params![input.account_id.to_string(), input.provider_message_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))?;
    let inserted = existing.is_none();
    let (message_id, created_at_ms) = existing
        .as_ref()
        .map_or((input.id, input.updated_at_ms), |(id, _, created)| {
            (MessageId::from_str(id).unwrap_or(input.id), *created)
        });
    if inserted {
        connection
            .execute(
                "INSERT INTO messages(
                    id, account_id, mailbox_id, provider_message_id, provider_revision,
                    thread_id, rfc_message_id, subject, snippet, body_plain, body_html,
                    is_read, direction, sent_at_ms, received_at_ms, parser_version,
                    sanitizer_version, created_at_ms, updated_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                           ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
                rusqlite::params_from_iter(message_params(input, message_id, created_at_ms)),
            )
            .map_err(|error| StorageError::from_sql(&error))?;
    } else {
        connection
            .execute(
                "UPDATE messages SET mailbox_id=?3, provider_revision=?5, thread_id=?6,
                    rfc_message_id=?7, subject=?8, snippet=?9, body_plain=?10,
                    body_html=?11, is_read=?12, direction=?13, sent_at_ms=?14,
                    received_at_ms=?15, parser_version=?16, sanitizer_version=?17,
                    updated_at_ms=?19 WHERE id=?1 AND account_id=?2",
                rusqlite::params_from_iter(message_params(input, message_id, created_at_ms)),
            )
            .map_err(|error| StorageError::from_sql(&error))?;
    }

    let id_text = message_id.to_string();
    queue_replaced_attachment_keys(connection, input.account_id, &id_text, &input.attachments)?;
    connection
        .execute(
            "DELETE FROM message_addresses WHERE message_id = ?1",
            [&id_text],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    for address in &input.addresses {
        insert_message_address(connection, &id_text, address)?;
    }
    connection
        .execute("DELETE FROM attachments WHERE message_id = ?1", [&id_text])
        .map_err(|error| StorageError::from_sql(&error))?;
    for attachment in &input.attachments {
        connection
            .execute(
                "INSERT INTO attachments(
                    id, message_id, provider_part_id, filename, media_type, size_bytes,
                    content_id, is_inline, cache_key, checksum_sha256
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    attachment.id.to_string(),
                    id_text,
                    attachment.provider_part_id,
                    attachment.file_name,
                    attachment.media_type,
                    attachment.size_bytes,
                    attachment.content_id,
                    attachment.inline,
                    attachment.cache_key,
                    attachment.checksum_sha256,
                ],
            )
            .map_err(|error| StorageError::from_sql(&error))?;
    }
    let row_id: i64 = connection
        .query_row(
            "SELECT row_id FROM messages WHERE id = ?1",
            [&id_text],
            |row| row.get(0),
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    connection
        .execute("DELETE FROM email_fts WHERE message_row_id = ?1", [row_id])
        .map_err(|error| StorageError::from_sql(&error))?;
    let sender = input
        .addresses
        .iter()
        .find(|address| address.role == AddressRole::From)
        .map_or_else(String::new, |address| {
            format!(
                "{} {}",
                address.display_name.as_deref().unwrap_or_default(),
                address.address
            )
        });
    let body = format!(
        "{} {}",
        input.plain_body.as_deref().unwrap_or_default(),
        strip_html(input.html_body.as_deref().unwrap_or_default())
    );
    connection
        .execute(
            "INSERT INTO email_fts(message_row_id, subject, body, sender) VALUES (?1, ?2, ?3, ?4)",
            params![
                row_id,
                search_document(input.subject.as_deref().unwrap_or_default()),
                search_document(&body),
                search_document(&sender)
            ],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    Ok(MessageUpsertResult {
        message_id,
        inserted,
    })
}

fn message_params(
    input: &MessageUpsertInput,
    message_id: MessageId,
    created_at_ms: i64,
) -> Vec<rusqlite::types::Value> {
    use rusqlite::types::Value;
    vec![
        Value::Text(message_id.to_string()),
        Value::Text(input.account_id.to_string()),
        Value::Text(input.mailbox_id.to_string()),
        Value::Text(input.provider_message_id.clone()),
        input
            .provider_revision
            .clone()
            .map_or(Value::Null, Value::Text),
        input.thread_id.clone().map_or(Value::Null, Value::Text),
        input
            .rfc_message_id
            .clone()
            .map_or(Value::Null, Value::Text),
        Value::Text(input.subject.clone().unwrap_or_default()),
        Value::Text(input.snippet.clone().unwrap_or_default()),
        input.plain_body.clone().map_or(Value::Null, Value::Text),
        input.html_body.clone().map_or(Value::Null, Value::Text),
        Value::Integer(i64::from(input.read)),
        Value::Text(direction_to_str(input.direction).to_owned()),
        input.sent_at_ms.map_or(Value::Null, Value::Integer),
        Value::Integer(input.received_at_ms),
        Value::Integer(i64::from(input.parser_version)),
        Value::Integer(i64::from(input.sanitizer_version)),
        Value::Integer(created_at_ms),
        Value::Integer(input.updated_at_ms),
    ]
}

fn insert_message_address(
    connection: &Connection,
    message_id: &str,
    address: &MessageAddressInput,
) -> Result<(), StorageError> {
    connection
        .execute(
            "INSERT INTO message_addresses(
                message_id, role, position, display_name, address
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                message_id,
                address_role_to_str(address.role),
                address.position,
                address.display_name,
                address.address,
            ],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    Ok(())
}

type SummaryRow = (
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    bool,
    String,
    Option<i64>,
    i64,
    bool,
);

fn list_messages(
    connection: &Connection,
    input: &MessageListInput,
) -> Result<MessagePage, StorageError> {
    let limit = input.limit.clamp(1, 100);
    let mailbox = input.mailbox_id.map(|id| id.to_string());
    let before_time = input.before.map(|cursor| cursor.received_at_ms);
    let before_id = input.before.map(|cursor| cursor.message_id.to_string());
    let mut statement = connection
        .prepare(
            "SELECT m.id, m.account_id, m.mailbox_id, m.subject, m.snippet,
                    (SELECT display_name FROM message_addresses a
                     WHERE a.message_id=m.id AND a.role='from' ORDER BY position LIMIT 1),
                    (SELECT address FROM message_addresses a
                     WHERE a.message_id=m.id AND a.role='from' ORDER BY position LIMIT 1),
                    m.is_read, m.direction, m.sent_at_ms, m.received_at_ms,
                    EXISTS(SELECT 1 FROM attachments x WHERE x.message_id=m.id)
             FROM messages m
             WHERE m.account_id=?1
               AND (?2 IS NULL OR m.mailbox_id=?2)
               AND (?3 IS NULL OR m.received_at_ms < ?3
                    OR (m.received_at_ms = ?3 AND m.id < ?4))
             ORDER BY m.received_at_ms DESC, m.id DESC LIMIT ?5",
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    let rows = statement
        .query_map(
            params![
                input.account_id.to_string(),
                mailbox,
                before_time,
                before_id,
                i64::from(limit) + 1,
            ],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                ))
            },
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    let mut items = rows
        .map(|row| {
            row.map_err(|error| StorageError::from_sql(&error))
                .and_then(summary_from_row)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let has_more = items.len() > limit as usize;
    items.truncate(limit as usize);
    let next = if has_more {
        items.last().map(|last| MessagePageCursor {
            received_at_ms: last.received_at_ms,
            message_id: last.id,
        })
    } else {
        None
    };
    Ok(MessagePage { items, next })
}

fn list_inbox_messages(
    connection: &Connection,
    input: &InboxListInput,
) -> Result<MessagePage, StorageError> {
    let limit = input.limit.clamp(1, 100);
    let account_id = input.account_id.map(|id| id.to_string());
    let before_time = input.before.map(|cursor| cursor.received_at_ms);
    let before_id = input.before.map(|cursor| cursor.message_id.to_string());
    let mut statement = connection
        .prepare(
            "SELECT m.id, m.account_id, m.mailbox_id, m.subject, m.snippet,
                    (SELECT display_name FROM message_addresses ma
                     WHERE ma.message_id=m.id AND ma.role='from' ORDER BY position LIMIT 1),
                    (SELECT address FROM message_addresses ma
                     WHERE ma.message_id=m.id AND ma.role='from' ORDER BY position LIMIT 1),
                    m.is_read, m.direction, m.sent_at_ms, m.received_at_ms,
                    EXISTS(SELECT 1 FROM attachments x WHERE x.message_id=m.id)
             FROM messages m
             JOIN mailboxes mb ON mb.id=m.mailbox_id AND mb.account_id=m.account_id
             JOIN accounts a ON a.id=m.account_id
             WHERE mb.role='inbox'
               AND a.enabled=1
               AND a.deleting=0
               AND (?1 IS NULL OR m.account_id=?1)
               AND (?2=0 OR m.is_read=0)
               AND (?3 IS NULL OR m.received_at_ms < ?3
                    OR (m.received_at_ms = ?3 AND m.id < ?4))
             ORDER BY m.received_at_ms DESC, m.id DESC LIMIT ?5",
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    let rows = statement
        .query_map(
            params![
                account_id,
                input.unread_only,
                before_time,
                before_id,
                i64::from(limit) + 1,
            ],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                ))
            },
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    let mut items = rows
        .map(|row| {
            row.map_err(|error| StorageError::from_sql(&error))
                .and_then(summary_from_row)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let has_more = items.len() > limit as usize;
    items.truncate(limit as usize);
    let next = if has_more {
        items.last().map(|last| MessagePageCursor {
            received_at_ms: last.received_at_ms,
            message_id: last.id,
        })
    } else {
        None
    };
    Ok(MessagePage { items, next })
}

type SearchRow = (SummaryRow, Option<String>, Option<String>, i64);

#[allow(clippy::too_many_lines)]
fn search_inbox_messages(
    connection: &Connection,
    input: &SearchMessagesInput,
) -> Result<SearchMessagePage, StorageError> {
    let fts_query = build_fts_query(&input.query)?;
    let limit = input.limit.clamp(1, 100);
    let account_id = input.account_id.map(|id| id.to_string());
    let after_rank = input.after.map(|cursor| cursor.rank_key);
    let after_time = input.after.map(|cursor| cursor.received_at_ms);
    let after_id = input.after.map(|cursor| cursor.message_id.to_string());
    let mut statement = connection
        .prepare(
            "WITH ranked AS (
                SELECT m.id, m.account_id, m.mailbox_id, m.subject, m.snippet,
                       (SELECT display_name FROM message_addresses ma
                        WHERE ma.message_id=m.id AND ma.role='from' ORDER BY position LIMIT 1)
                           AS sender_name,
                       (SELECT address FROM message_addresses ma
                        WHERE ma.message_id=m.id AND ma.role='from' ORDER BY position LIMIT 1)
                           AS sender_address,
                       m.is_read, m.direction, m.sent_at_ms, m.received_at_ms,
                       EXISTS(SELECT 1 FROM attachments x WHERE x.message_id=m.id)
                           AS has_attachments,
                       m.body_plain, m.body_html,
                       CAST(bm25(email_fts, 0.0, 8.0, 1.0, 4.0) * 1000000000.0 AS INTEGER) AS rank_key
                FROM email_fts
                JOIN messages m ON m.row_id=email_fts.message_row_id
                JOIN mailboxes mb ON mb.id=m.mailbox_id AND mb.account_id=m.account_id
                JOIN accounts a ON a.id=m.account_id
                WHERE email_fts MATCH ?1
                  AND mb.role='inbox'
                  AND a.enabled=1
                  AND a.deleting=0
                  AND (?2 IS NULL OR m.account_id=?2)
                  AND (?3=0 OR m.is_read=0)
            )
            SELECT id, account_id, mailbox_id, subject, snippet,
                   sender_name, sender_address, is_read, direction, sent_at_ms,
                   received_at_ms, has_attachments, body_plain, body_html, rank_key
            FROM ranked
            WHERE ?4 IS NULL OR rank_key > ?4
               OR (rank_key = ?4 AND (received_at_ms < ?5
                   OR (received_at_ms = ?5 AND id < ?6)))
            ORDER BY rank_key ASC, received_at_ms DESC, id DESC
            LIMIT ?7",
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    let rows = statement
        .query_map(
            params![
                fts_query,
                account_id,
                input.unread_only,
                after_rank,
                after_time,
                after_id,
                i64::from(limit) + 1,
            ],
            |row| {
                Ok((
                    (
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                        row.get(10)?,
                        row.get(11)?,
                    ),
                    row.get(12)?,
                    row.get(13)?,
                    row.get(14)?,
                ))
            },
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    let mut items = rows
        .map(|row| {
            let row: SearchRow = row.map_err(|error| StorageError::from_sql(&error))?;
            let summary = summary_from_row(row.0)?;
            let match_context = search_match_context(
                &input.query,
                summary.subject.as_deref(),
                summary.sender_name.as_deref(),
                summary.sender_address.as_deref(),
                row.1.as_deref(),
                row.2.as_deref(),
            );
            Ok(SearchMessageHit {
                summary,
                match_context,
                rank_key: row.3,
            })
        })
        .collect::<Result<Vec<_>, StorageError>>()?;
    let has_more = items.len() > limit as usize;
    items.truncate(limit as usize);
    let scope_hash =
        unimail_core::search_scope_hash(&input.query, input.account_id, input.unread_only);
    let next = if has_more {
        items.last().map(|last| SearchMessageCursor {
            scope_hash,
            rank_key: last.rank_key,
            received_at_ms: last.summary.received_at_ms,
            message_id: last.summary.id,
        })
    } else {
        None
    };
    Ok(SearchMessagePage { items, next })
}

fn build_fts_query(query: &str) -> Result<String, StorageError> {
    let terms = search_tokens(query, MAX_SEARCH_TERMS + 1);
    if terms.is_empty() || terms.len() > MAX_SEARCH_TERMS {
        return Err(StorageError::Serialization);
    }
    Ok(terms
        .into_iter()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND "))
}

fn search_document(value: &str) -> String {
    search_tokens(value, 4096).join(" ")
}

fn search_tokens(value: &str, maximum: usize) -> Vec<String> {
    let normalized = value
        .nfkc()
        .flat_map(char::to_lowercase)
        .collect::<String>();
    let mut terms = Vec::new();
    let mut ordinary = String::new();
    let mut cjk = Vec::new();

    let flush_ordinary = |ordinary: &mut String, terms: &mut Vec<String>| {
        if !ordinary.is_empty() && terms.len() < maximum {
            terms.push(std::mem::take(ordinary));
        }
    };
    let flush_cjk = |cjk: &mut Vec<char>, terms: &mut Vec<String>| {
        if cjk.is_empty() {
            return;
        }
        for width in 1..=3 {
            for window in cjk.windows(width) {
                if terms.len() >= maximum {
                    cjk.clear();
                    return;
                }
                let token = window.iter().collect::<String>();
                if !terms.contains(&token) {
                    terms.push(token);
                }
            }
        }
        cjk.clear();
    };

    for character in normalized.chars() {
        if is_cjk(character) {
            flush_ordinary(&mut ordinary, &mut terms);
            cjk.push(character);
        } else if character.is_alphanumeric() {
            flush_cjk(&mut cjk, &mut terms);
            ordinary.push(character);
        } else {
            flush_ordinary(&mut ordinary, &mut terms);
            flush_cjk(&mut cjk, &mut terms);
        }
        if terms.len() >= maximum {
            break;
        }
    }
    flush_ordinary(&mut ordinary, &mut terms);
    flush_cjk(&mut cjk, &mut terms);
    terms.truncate(maximum);
    terms
}

fn is_cjk(character: char) -> bool {
    matches!(
        character as u32,
        0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0x3040..=0x30FF
            | 0xAC00..=0xD7AF
    )
}

fn search_match_context(
    query: &str,
    subject: Option<&str>,
    sender_name: Option<&str>,
    sender_address: Option<&str>,
    plain_body: Option<&str>,
    html_body: Option<&str>,
) -> Option<String> {
    let query_terms = search_tokens(query, MAX_SEARCH_TERMS);
    let sender = format!(
        "{} {}",
        sender_name.unwrap_or_default(),
        sender_address.unwrap_or_default()
    );
    let candidates = [
        subject.unwrap_or_default().to_owned(),
        sender,
        plain_body.unwrap_or_default().to_owned(),
        strip_html(html_body.unwrap_or_default()),
    ];
    let selected = candidates.iter().find(|candidate| {
        let normalized_candidate = candidate
            .nfkc()
            .flat_map(char::to_lowercase)
            .collect::<String>();
        query_terms
            .iter()
            .any(|term| normalized_candidate.contains(term))
    })?;
    let collapsed = selected.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    let mut characters = collapsed.chars();
    let prefix = characters
        .by_ref()
        .take(MAX_SEARCH_CONTEXT_CHARS)
        .collect::<String>();
    if characters.next().is_some() {
        Some(format!("{prefix}…"))
    } else {
        Some(prefix)
    }
}

fn strip_html(value: &str) -> String {
    let mut plain = String::with_capacity(value.len());
    let mut in_tag = false;
    for character in value.chars() {
        match character {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                plain.push(' ');
            }
            _ if !in_tag => plain.push(character),
            _ => {}
        }
    }
    plain
}

fn rebuild_search_index(connection: &Connection) -> Result<(), StorageError> {
    type SearchDocumentRow = (i64, String, Option<String>, Option<String>, String);
    let rows = {
        let mut statement = connection
            .prepare(
                "SELECT m.row_id, m.subject, m.body_plain, m.body_html,
                        coalesce((SELECT coalesce(a.display_name, '') || ' ' || a.address
                                  FROM message_addresses a
                                  WHERE a.message_id=m.id AND a.role IN ('from', 'sender')
                                  ORDER BY CASE a.role WHEN 'from' THEN 0 ELSE 1 END,
                                           a.position LIMIT 1), '')
                 FROM messages m ORDER BY m.row_id",
            )
            .map_err(|error| StorageError::from_sql(&error))?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .map_err(|error| StorageError::from_sql(&error))?
            .map(|row| row.map_err(|error| StorageError::from_sql(&error)))
            .collect::<Result<Vec<SearchDocumentRow>, StorageError>>()?
    };
    connection
        .execute("DELETE FROM email_fts", [])
        .map_err(|error| StorageError::from_sql(&error))?;
    for (row_id, subject, plain_body, html_body, sender) in rows {
        let body = format!(
            "{} {}",
            plain_body.as_deref().unwrap_or_default(),
            strip_html(html_body.as_deref().unwrap_or_default())
        );
        connection
            .execute(
                "INSERT INTO email_fts(message_row_id, subject, body, sender)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    row_id,
                    search_document(&subject),
                    search_document(&body),
                    search_document(&sender),
                ],
            )
            .map_err(|error| StorageError::from_sql(&error))?;
    }
    connection
        .execute(
            "UPDATE search_index_state SET document_version=?1 WHERE singleton=1",
            [SEARCH_DOCUMENT_VERSION],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    Ok(())
}

fn summary_from_row(row: SummaryRow) -> Result<MessageSummary, StorageError> {
    Ok(MessageSummary {
        id: parse_id(&row.0)?,
        account_id: parse_id(&row.1)?,
        mailbox_id: parse_id(&row.2)?,
        subject: nonempty(row.3),
        snippet: nonempty(row.4),
        sender_name: row.5,
        sender_address: row.6,
        read: row.7,
        direction: direction_from_str(&row.8)?,
        sent_at_ms: row.9,
        received_at_ms: row.10,
        has_attachments: row.11,
    })
}

fn get_attachment_download_source(
    connection: &Connection,
    attachment_id: AttachmentId,
) -> Result<Option<AttachmentDownloadSource>, StorageError> {
    type SourceRow = (
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        Option<String>,
        String,
        Option<i64>,
        Option<String>,
    );
    connection
        .query_row(
            "SELECT x.id, x.message_id, m.account_id, a.provider,
                    mb.provider_mailbox_id, m.provider_message_id, x.provider_part_id,
                    x.filename, x.media_type, x.size_bytes, x.checksum_sha256
             FROM attachments x
             JOIN messages m ON m.id=x.message_id
             JOIN mailboxes mb ON mb.id=m.mailbox_id AND mb.account_id=m.account_id
             JOIN accounts a ON a.id=m.account_id
             WHERE x.id=?1
               AND x.is_inline=0
               AND x.provider_part_id IS NOT NULL
               AND x.provider_part_id<>''
               AND mb.role='inbox'
               AND a.enabled=1
               AND a.deleting=0",
            [attachment_id.to_string()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                ))
            },
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))?
        .map(|row: SourceRow| {
            let account_id = parse_id(&row.2)?;
            let size_bytes = row
                .9
                .map(|value| u64::try_from(value).map_err(|_| StorageError::Serialization))
                .transpose()?;
            Ok(AttachmentDownloadSource {
                attachment_id: parse_id(&row.0)?,
                message_id: parse_id(&row.1)?,
                account_id,
                provider: provider_from_str(&row.3)?,
                key: RemoteMessageKey {
                    account_id,
                    provider_mailbox_id: row.4,
                    provider_message_id: row.5,
                },
                provider_part_id: row.6,
                file_name: row.7,
                media_type: row.8,
                size_bytes,
                checksum_sha256: row.10,
            })
        })
        .transpose()
}

fn record_attachment_verification(
    connection: &Connection,
    input: &AttachmentVerificationInput,
) -> Result<(), StorageError> {
    if !is_sha256(&input.checksum_sha256) {
        return Err(StorageError::Serialization);
    }
    let size_bytes = i64::try_from(input.size_bytes).map_err(|_| StorageError::Constraint)?;
    let existing = connection
        .query_row(
            "SELECT size_bytes, checksum_sha256 FROM attachments WHERE id=?1",
            [input.attachment_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            },
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))?
        .ok_or(StorageError::NotFound)?;
    if existing.0.is_some_and(|value| value != size_bytes)
        || existing
            .1
            .as_deref()
            .is_some_and(|value| !value.eq_ignore_ascii_case(&input.checksum_sha256))
    {
        return Err(StorageError::Constraint);
    }
    connection
        .execute(
            "UPDATE attachments
             SET size_bytes=?2, checksum_sha256=?3
             WHERE id=?1",
            params![
                input.attachment_id.to_string(),
                size_bytes,
                input.checksum_sha256.to_ascii_lowercase(),
            ],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn get_message(
    connection: &Connection,
    message_id: MessageId,
) -> Result<Option<MessageDetail>, StorageError> {
    type DetailRow = (
        SummaryRow,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        u32,
        u32,
    );
    let raw: Option<DetailRow> = connection
        .query_row(
            "SELECT m.id, m.account_id, m.mailbox_id, m.subject, m.snippet,
                    (SELECT display_name FROM message_addresses a WHERE a.message_id=m.id AND a.role='from' ORDER BY position LIMIT 1),
                    (SELECT address FROM message_addresses a WHERE a.message_id=m.id AND a.role='from' ORDER BY position LIMIT 1),
                    m.is_read, m.direction, m.sent_at_ms, m.received_at_ms,
                    EXISTS(SELECT 1 FROM attachments x WHERE x.message_id=m.id),
                    m.thread_id, m.rfc_message_id, m.body_plain, m.body_html,
                    m.parser_version, m.sanitizer_version
             FROM messages m WHERE m.id=?1",
            [message_id.to_string()],
            |row| {
                Ok(((
                    row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?,
                    row.get(5)?, row.get(6)?, row.get(7)?, row.get(8)?, row.get(9)?,
                    row.get(10)?, row.get(11)?,
                ), row.get(12)?, row.get(13)?, row.get(14)?, row.get(15)?, row.get(16)?, row.get(17)?))
            },
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))?;
    let Some((
        summary,
        thread_id,
        rfc_message_id,
        plain_body,
        html_body,
        parser_version,
        sanitizer_version,
    )) = raw
    else {
        return Ok(None);
    };
    let addresses = load_message_addresses(connection, message_id)?;
    let attachments = load_attachments(connection, message_id)?;
    Ok(Some(MessageDetail {
        summary: summary_from_row(summary)?,
        thread_id,
        rfc_message_id,
        plain_body,
        html_body,
        parser_version,
        sanitizer_version,
        addresses,
        attachments,
    }))
}

fn get_reply_source(
    connection: &Connection,
    message_id: MessageId,
) -> Result<Option<ReplySource>, StorageError> {
    type ReplyRow = (
        String,
        String,
        Option<String>,
        Option<String>,
        String,
        String,
        Option<String>,
        i64,
        Option<String>,
        Option<String>,
    );
    let row: Option<ReplyRow> = connection
        .query_row(
            "SELECT m.account_id, m.provider_message_id, m.thread_id, m.rfc_message_id,
                    m.references_json, m.subject, m.body_plain, m.received_at_ms,
                    (SELECT display_name FROM message_addresses a
                     WHERE a.message_id=m.id AND a.role='from' ORDER BY position LIMIT 1),
                    (SELECT address FROM message_addresses a
                     WHERE a.message_id=m.id AND a.role='from' ORDER BY position LIMIT 1)
             FROM messages m WHERE m.id=?1",
            [message_id.to_string()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                ))
            },
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))?;
    let Some((
        account_id,
        original_provider_message_id,
        provider_thread_id,
        rfc_message_id,
        references_json,
        subject,
        plain_body,
        received_at_ms,
        sender_name,
        sender_address,
    )) = row
    else {
        return Ok(None);
    };
    let sender_address = sender_address
        .filter(|value| !value.trim().is_empty())
        .ok_or(StorageError::Serialization)?;
    let references: Vec<String> =
        serde_json::from_str(&references_json).map_err(|_| StorageError::Serialization)?;
    if references.iter().any(|value| value.trim().is_empty())
        || original_provider_message_id.trim().is_empty()
    {
        return Err(StorageError::Serialization);
    }
    Ok(Some(ReplySource {
        message_id,
        account_id: parse_id(&account_id)?,
        provider_thread_id,
        original_provider_message_id,
        rfc_message_id,
        references,
        sender: DraftAddress {
            display_name: sender_name,
            address: sender_address,
        },
        subject,
        plain_body,
        received_at_ms,
    }))
}

fn load_message_addresses(
    connection: &Connection,
    message_id: MessageId,
) -> Result<Vec<MessageAddress>, StorageError> {
    let mut statement = connection
        .prepare("SELECT role, position, display_name, address FROM message_addresses WHERE message_id=?1 ORDER BY role, position")
        .map_err(|error| StorageError::from_sql(&error))?;
    let rows = statement
        .query_map([message_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
            ))
        })
        .map_err(|error| StorageError::from_sql(&error))?;
    rows.map(|row| {
        let (role, position, display_name, address) =
            row.map_err(|error| StorageError::from_sql(&error))?;
        Ok(MessageAddress {
            role: address_role_from_str(&role)?,
            position,
            display_name,
            address,
        })
    })
    .collect()
}

fn load_attachments(
    connection: &Connection,
    message_id: MessageId,
) -> Result<Vec<Attachment>, StorageError> {
    let mut statement = connection
        .prepare("SELECT id, provider_part_id, filename, media_type, size_bytes, content_id, is_inline, cache_key, checksum_sha256 FROM attachments WHERE message_id=?1 ORDER BY rowid")
        .map_err(|error| StorageError::from_sql(&error))?;
    let rows = statement
        .query_map([message_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
            ))
        })
        .map_err(|error| StorageError::from_sql(&error))?;
    rows.map(|row| {
        let (
            id,
            provider_part_id,
            file_name,
            media_type,
            size_bytes,
            content_id,
            inline,
            cache_key,
            checksum_sha256,
        ) = row.map_err(|error| StorageError::from_sql(&error))?;
        Ok(Attachment {
            id: parse_id(&id)?,
            message_id,
            provider_part_id,
            file_name,
            media_type,
            size_bytes,
            content_id,
            inline,
            cache_key,
            checksum_sha256,
        })
    })
    .collect()
}

fn save_draft(connection: &Connection, input: &DraftSaveInput) -> Result<Draft, StorageError> {
    let current: Option<(i64, i64)> = connection
        .query_row(
            "SELECT revision, created_at_ms FROM drafts WHERE id=?1",
            [input.id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))?;
    let (revision, created_at_ms) = match current {
        None if input.expected_revision.is_none() => (1, input.updated_at_ms),
        Some((revision, created_at_ms))
            if input.expected_revision == u64::try_from(revision).ok() =>
        {
            (revision + 1, created_at_ms)
        }
        None | Some(_) => return Err(StorageError::DraftRevisionConflict),
    };
    let recipients = serde_json::json!({
        "to": encode_draft_addresses(&input.to),
        "cc": encode_draft_addresses(&input.cc),
        "bcc": encode_draft_addresses(&input.bcc),
    });
    connection
        .execute(
            "INSERT INTO drafts(
                id, account_id, subject, body_plain, body_html, recipients_json,
                in_reply_to, revision, created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(id) DO UPDATE SET
                account_id=excluded.account_id, subject=excluded.subject,
                body_plain=excluded.body_plain, body_html=excluded.body_html,
                recipients_json=excluded.recipients_json, in_reply_to=excluded.in_reply_to,
                revision=excluded.revision, updated_at_ms=excluded.updated_at_ms",
            params![
                input.id.to_string(),
                input.account_id.to_string(),
                input.subject,
                input.plain_body,
                input.html_body,
                recipients.to_string(),
                input.in_reply_to_message_id.map(|id| id.to_string()),
                revision,
                created_at_ms,
                input.updated_at_ms,
            ],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    connection
        .execute(
            "DELETE FROM draft_attachments WHERE draft_id=?1",
            [input.id.to_string()],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    for (position, attachment) in input.attachments.iter().enumerate() {
        connection
            .execute(
                "INSERT INTO draft_attachments(
                    id, draft_id, filename, media_type, size_bytes, local_ref, position
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    attachment.id.to_string(),
                    input.id.to_string(),
                    attachment.file_name,
                    attachment.media_type,
                    attachment.size_bytes,
                    attachment.local_file_ref,
                    i64::try_from(position).map_err(|_| StorageError::Constraint)?,
                ],
            )
            .map_err(|error| StorageError::from_sql(&error))?;
    }
    connection
        .execute(
            "DELETE FROM draft_send_reviews WHERE draft_id=?1 AND draft_revision<>?2",
            params![input.id.to_string(), revision],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    Ok(Draft {
        id: input.id,
        account_id: input.account_id,
        to: input.to.clone(),
        cc: input.cc.clone(),
        bcc: input.bcc.clone(),
        subject: input.subject.clone(),
        plain_body: input.plain_body.clone(),
        html_body: input.html_body.clone(),
        in_reply_to_message_id: input.in_reply_to_message_id,
        attachments: input.attachments.clone(),
        revision: u64::try_from(revision).map_err(|_| StorageError::Serialization)?,
        created_at_ms,
        updated_at_ms: input.updated_at_ms,
    })
}

fn get_draft(connection: &Connection, draft_id: DraftId) -> Result<Option<Draft>, StorageError> {
    type DraftRow = (
        String,
        String,
        String,
        Option<String>,
        String,
        Option<String>,
        i64,
        i64,
        i64,
    );
    let row: Option<DraftRow> = connection
        .query_row(
            "SELECT account_id, subject, body_plain, body_html, recipients_json,
                    in_reply_to, revision, created_at_ms, updated_at_ms
             FROM drafts WHERE id=?1",
            [draft_id.to_string()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            },
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))?;
    let Some((
        account_id,
        subject,
        plain_body,
        html_body,
        recipients,
        in_reply_to,
        revision,
        created_at_ms,
        updated_at_ms,
    )) = row
    else {
        return Ok(None);
    };
    let value: serde_json::Value =
        serde_json::from_str(&recipients).map_err(|_| StorageError::Serialization)?;
    let addresses = |name: &str| decode_draft_addresses(value.get(name));
    Ok(Some(Draft {
        id: draft_id,
        account_id: parse_id(&account_id)?,
        to: addresses("to")?,
        cc: addresses("cc")?,
        bcc: addresses("bcc")?,
        subject,
        plain_body,
        html_body,
        in_reply_to_message_id: in_reply_to.map(|id| parse_id(&id)).transpose()?,
        attachments: load_draft_attachments(connection, draft_id)?,
        revision: u64::try_from(revision).map_err(|_| StorageError::Serialization)?,
        created_at_ms,
        updated_at_ms,
    }))
}

fn load_draft_attachments(
    connection: &Connection,
    draft_id: DraftId,
) -> Result<Vec<DraftAttachmentInput>, StorageError> {
    let mut statement = connection
        .prepare("SELECT id, filename, media_type, size_bytes, local_ref FROM draft_attachments WHERE draft_id=?1 ORDER BY position")
        .map_err(|error| StorageError::from_sql(&error))?;
    let rows = statement
        .query_map([draft_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .map_err(|error| StorageError::from_sql(&error))?;
    rows.map(|row| {
        let (id, file_name, media_type, size_bytes, local_file_ref) =
            row.map_err(|error| StorageError::from_sql(&error))?;
        Ok(DraftAttachmentInput {
            id: parse_id(&id)?,
            file_name,
            media_type,
            size_bytes,
            local_file_ref,
        })
    })
    .collect()
}

fn encode_draft_addresses(addresses: &[DraftAddress]) -> serde_json::Value {
    serde_json::Value::Array(
        addresses
            .iter()
            .map(|address| {
                serde_json::json!({
                    "displayName": address.display_name,
                    "address": address.address,
                })
            })
            .collect(),
    )
}

fn decode_draft_addresses(
    value: Option<&serde_json::Value>,
) -> Result<Vec<DraftAddress>, StorageError> {
    let values = value
        .and_then(serde_json::Value::as_array)
        .ok_or(StorageError::Serialization)?;
    values
        .iter()
        .map(|value| {
            let object = value.as_object().ok_or(StorageError::Serialization)?;
            let display_name = object
                .get("displayName")
                .filter(|value| !value.is_null())
                .map(|value| {
                    value
                        .as_str()
                        .map(ToOwned::to_owned)
                        .ok_or(StorageError::Serialization)
                })
                .transpose()?;
            let address = object
                .get("address")
                .and_then(serde_json::Value::as_str)
                .ok_or(StorageError::Serialization)?
                .to_owned();
            Ok(DraftAddress {
                display_name,
                address,
            })
        })
        .collect()
}

fn list_drafts(
    connection: &Connection,
    account_id: Option<AccountId>,
) -> Result<Vec<DraftSummary>, StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT id, account_id, subject, recipients_json, revision, updated_at_ms
             FROM drafts WHERE (?1 IS NULL OR account_id=?1)
             ORDER BY updated_at_ms DESC, id DESC",
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    let account = account_id.map(|id| id.to_string());
    let rows = statement
        .query_map([account], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .map_err(|error| StorageError::from_sql(&error))?;
    rows.map(|row| {
        let (id, account_id, subject, recipients, revision, updated_at_ms) =
            row.map_err(|error| StorageError::from_sql(&error))?;
        let value: serde_json::Value =
            serde_json::from_str(&recipients).map_err(|_| StorageError::Serialization)?;
        let recipient_count = ["to", "cc", "bcc"]
            .iter()
            .map(|key| {
                value
                    .get(key)
                    .and_then(serde_json::Value::as_array)
                    .map_or(0, Vec::len)
            })
            .sum::<usize>();
        Ok(DraftSummary {
            id: parse_id(&id)?,
            account_id: parse_id(&account_id)?,
            subject,
            recipient_count: u32::try_from(recipient_count)
                .map_err(|_| StorageError::Serialization)?,
            revision: u64::try_from(revision).map_err(|_| StorageError::Serialization)?,
            updated_at_ms,
        })
    })
    .collect()
}

fn get_sync_cursor(
    connection: &Connection,
    key: &SyncCursorKey,
) -> Result<Option<SyncCursor>, StorageError> {
    let row: Option<(String, i64, i64)> = connection
        .query_row(
            "SELECT checkpoint_json, updated_at_ms, last_successful_at_ms
             FROM sync_cursors WHERE account_id=?1 AND scope=?2",
            params![key.account_id.to_string(), key.scope],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))?;
    row.map(|(encoded, updated_at_ms, last_successful_at_ms)| {
        let checkpoint = OpaqueProviderCursor::from_json(encoded)
            .map(DurableCheckpoint::new)
            .map_err(|_| StorageError::Serialization)?;
        Ok(SyncCursor {
            key: key.clone(),
            checkpoint,
            updated_at_ms,
            last_successful_sync_at_ms: Some(last_successful_at_ms),
        })
    })
    .transpose()
}

fn store_sync_cursor(connection: &Connection, cursor: &SyncCursor) -> Result<(), StorageError> {
    let last_successful_at_ms = cursor
        .last_successful_sync_at_ms
        .unwrap_or(cursor.updated_at_ms);
    connection
        .execute(
            "INSERT INTO sync_cursors(
                account_id, scope, checkpoint_json, updated_at_ms, last_successful_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(account_id, scope) DO UPDATE SET
                checkpoint_json=excluded.checkpoint_json,
                updated_at_ms=max(sync_cursors.updated_at_ms, excluded.updated_at_ms),
                last_successful_at_ms=max(
                    sync_cursors.last_successful_at_ms,
                    excluded.last_successful_at_ms
                )",
            params![
                cursor.key.account_id.to_string(),
                cursor.key.scope,
                cursor.checkpoint.cursor().as_json(),
                cursor.updated_at_ms,
                last_successful_at_ms,
            ],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    Ok(())
}

fn set_message_read(
    connection: &Connection,
    input: MessageReadStateInput,
) -> Result<DesiredReadMutation, StorageError> {
    let row: Option<DesiredReadSourceRow> = connection
        .query_row(
            "SELECT m.account_id, mb.provider_mailbox_id, m.provider_message_id,
                    m.provider_revision,
                    r.read_intent_generation, m.created_at_ms, m.updated_at_ms
             FROM messages m
             JOIN mailboxes mb ON mb.id=m.mailbox_id AND mb.account_id=m.account_id
             JOIN remote_message_ids r ON r.message_id=m.id
             WHERE m.id=?1",
            [input.message_id.to_string()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))?;
    let Some((
        account,
        mailbox,
        provider_message,
        revision,
        generation,
        created_at_ms,
        current_updated_at_ms,
    )) = row
    else {
        return Err(StorageError::Constraint);
    };
    let updated_at_ms = input
        .updated_at_ms
        .max(created_at_ms)
        .max(current_updated_at_ms);
    connection
        .execute(
            "UPDATE messages SET is_read=?2, updated_at_ms=max(updated_at_ms, created_at_ms, ?3)
             WHERE id=?1",
            params![input.message_id.to_string(), input.read, updated_at_ms],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    let next_generation = generation.checked_add(1).ok_or(StorageError::Constraint)?;
    let generation_changed = connection
        .execute(
            "UPDATE remote_message_ids SET read_intent_generation=?4
             WHERE account_id=?1 AND provider_mailbox_id=?2 AND provider_message_id=?3
               AND read_intent_generation=?5",
            params![
                account,
                mailbox,
                provider_message,
                next_generation,
                generation
            ],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    if generation_changed != 1 {
        return Err(StorageError::Constraint);
    }
    connection
        .execute(
            "INSERT INTO pending_read_mutations(
                account_id, provider_mailbox_id, provider_message_id, message_id,
                desired_read, expected_provider_revision, intent_generation, state,
                attempt_count, created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', 0, ?8, ?9)
             ON CONFLICT(account_id, provider_mailbox_id, provider_message_id) DO UPDATE SET
                message_id=excluded.message_id, desired_read=excluded.desired_read,
                expected_provider_revision=excluded.expected_provider_revision,
                intent_generation=excluded.intent_generation, state='pending',
                attempt_count=0, next_attempt_at_ms=NULL, lease_id=NULL,
                lease_expires_at_ms=NULL, safe_error_code=NULL,
                updated_at_ms=max(pending_read_mutations.updated_at_ms, excluded.updated_at_ms)",
            params![
                account,
                mailbox,
                provider_message,
                input.message_id.to_string(),
                input.read,
                revision,
                next_generation,
                created_at_ms,
                updated_at_ms,
            ],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    load_desired_read_mutation(connection, input.message_id)?.ok_or(StorageError::Constraint)
}

fn load_desired_read_mutation(
    connection: &Connection,
    message_id: MessageId,
) -> Result<Option<DesiredReadMutation>, StorageError> {
    connection
        .query_row(
            "SELECT account_id, provider_mailbox_id, provider_message_id, message_id,
                    desired_read, expected_provider_revision, intent_generation, state,
                    attempt_count, next_attempt_at_ms, lease_id, lease_expires_at_ms,
                    safe_error_code, created_at_ms, updated_at_ms
             FROM pending_read_mutations WHERE message_id=?1",
            [message_id.to_string()],
            desired_read_from_row,
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))
}

fn desired_read_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DesiredReadMutation> {
    let account_id = parse_id_sql(&row.get::<_, String>(0)?)?;
    let message_id = parse_id_sql(&row.get::<_, String>(3)?)?;
    let generation_raw = row.get::<_, i64>(6)?;
    let generation = u64::try_from(generation_raw)
        .ok()
        .and_then(ReadIntentGeneration::new)
        .ok_or_else(|| {
            rusqlite::Error::InvalidColumnType(
                6,
                "intent_generation".into(),
                rusqlite::types::Type::Integer,
            )
        })?;
    let state_raw = row.get::<_, String>(7)?;
    let state = desired_read_state_from_str(&state_raw).map_err(storage_to_sql)?;
    let lease_id = row.get::<_, Option<String>>(10)?;
    let lease_expires_at_ms = row.get::<_, Option<i64>>(11)?;
    let lease = match (lease_id, lease_expires_at_ms) {
        (Some(id), Some(expires_at_ms)) => Some(OperationLease {
            id: parse_id_sql(&id)?,
            expires_at_ms,
        }),
        (None, None) => None,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    let safe_error_code = row
        .get::<_, Option<String>>(12)?
        .map(|value| SafeErrorCode::new(value).ok_or(rusqlite::Error::InvalidQuery))
        .transpose()?;
    Ok(DesiredReadMutation {
        key: RemoteMessageKey {
            account_id,
            provider_mailbox_id: row.get(1)?,
            provider_message_id: row.get(2)?,
        },
        message_id,
        desired_read: row.get(4)?,
        expected_revision: row.get::<_, Option<String>>(5)?.map(ProviderRevision::new),
        generation,
        state,
        attempt_count: u32::try_from(row.get::<_, i64>(8)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        next_attempt_at_ms: row.get(9)?,
        lease,
        safe_error_code,
        created_at_ms: row.get(13)?,
        updated_at_ms: row.get(14)?,
    })
}

fn list_due_desired_read_mutations(
    connection: &Connection,
    account_id: AccountId,
    now_ms: i64,
    limit: u32,
) -> Result<Vec<DesiredReadMutation>, StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT account_id, provider_mailbox_id, provider_message_id, message_id,
                    desired_read, expected_provider_revision, intent_generation, state,
                    attempt_count, next_attempt_at_ms, lease_id, lease_expires_at_ms,
                    safe_error_code, created_at_ms, updated_at_ms
             FROM pending_read_mutations
             WHERE account_id=?1 AND state IN ('pending', 'waiting_backoff')
               AND (next_attempt_at_ms IS NULL OR next_attempt_at_ms<=?2 OR ?2<updated_at_ms)
             ORDER BY coalesce(next_attempt_at_ms, created_at_ms), created_at_ms,
                      provider_mailbox_id, provider_message_id LIMIT ?3",
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    statement
        .query_map(
            params![
                account_id.to_string(),
                now_ms,
                i64::from(limit.clamp(1, 100))
            ],
            desired_read_from_row,
        )
        .map_err(|error| StorageError::from_sql(&error))?
        .map(|row| row.map_err(|error| StorageError::from_sql(&error)))
        .collect()
}

fn claim_desired_read_mutation(
    connection: &Connection,
    input: &ClaimDesiredReadMutationInput,
) -> Result<Option<DesiredReadMutation>, StorageError> {
    let current: Option<(Option<i64>, i64, i64)> = connection
        .query_row(
            "SELECT next_attempt_at_ms, created_at_ms, updated_at_ms
             FROM pending_read_mutations
             WHERE account_id=?1 AND provider_mailbox_id=?2 AND provider_message_id=?3
               AND intent_generation=?4 AND state IN ('pending', 'waiting_backoff')",
            params![
                input.key.account_id.to_string(),
                input.key.provider_mailbox_id,
                input.key.provider_message_id,
                i64::try_from(input.generation.get()).map_err(|_| StorageError::Constraint)?,
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))?;
    let Some((next_attempt_at_ms, created_at_ms, updated_at_ms)) = current else {
        return Ok(None);
    };
    if next_attempt_at_ms.is_some_and(|deadline| {
        deadline > input.claimed_at_ms && input.claimed_at_ms >= updated_at_ms
    }) {
        return Ok(None);
    }
    let claimed_at_ms = clamp_durable_time(input.claimed_at_ms, created_at_ms, updated_at_ms);
    let lease = clamp_lease(input.lease, input.claimed_at_ms, claimed_at_ms);
    let changed = connection
        .execute(
            "UPDATE pending_read_mutations
             SET state='running', lease_id=?5, lease_expires_at_ms=?6,
                 next_attempt_at_ms=NULL, updated_at_ms=?7
             WHERE account_id=?1 AND provider_mailbox_id=?2 AND provider_message_id=?3
               AND intent_generation=?4 AND state IN ('pending', 'waiting_backoff')
               AND (next_attempt_at_ms IS NULL OR next_attempt_at_ms<=?8 OR ?8<updated_at_ms)",
            params![
                input.key.account_id.to_string(),
                input.key.provider_mailbox_id,
                input.key.provider_message_id,
                i64::try_from(input.generation.get()).map_err(|_| StorageError::Constraint)?,
                lease.id.to_string(),
                lease.expires_at_ms,
                claimed_at_ms,
                input.claimed_at_ms,
            ],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    if changed == 0 {
        return Ok(None);
    }
    connection
        .query_row(
            "SELECT account_id, provider_mailbox_id, provider_message_id, message_id,
                    desired_read, expected_provider_revision, intent_generation, state,
                    attempt_count, next_attempt_at_ms, lease_id, lease_expires_at_ms,
                    safe_error_code, created_at_ms, updated_at_ms
             FROM pending_read_mutations
             WHERE account_id=?1 AND provider_mailbox_id=?2 AND provider_message_id=?3",
            params![
                input.key.account_id.to_string(),
                input.key.provider_mailbox_id,
                input.key.provider_message_id,
            ],
            desired_read_from_row,
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))
}

fn complete_desired_read_mutation(
    connection: &Connection,
    input: &CompleteDesiredReadMutationInput,
) -> Result<bool, StorageError> {
    let desired: Option<bool> = connection
        .query_row(
            "SELECT desired_read FROM pending_read_mutations
             WHERE account_id=?1 AND provider_mailbox_id=?2 AND provider_message_id=?3
               AND intent_generation=?4 AND lease_id=?5 AND state='running'",
            params![
                input.key.account_id.to_string(),
                input.key.provider_mailbox_id,
                input.key.provider_message_id,
                i64::try_from(input.generation.get()).map_err(|_| StorageError::Constraint)?,
                input.lease_id.to_string(),
            ],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))?;
    if desired != Some(input.provider_read) {
        return Ok(false);
    }
    connection
        .execute(
            "UPDATE messages SET remote_is_read=?4, is_read=?4, provider_revision=?5,
                    updated_at_ms=max(updated_at_ms, created_at_ms, ?6)
             WHERE id=(SELECT message_id FROM remote_message_ids
                       WHERE account_id=?1 AND provider_mailbox_id=?2 AND provider_message_id=?3)",
            params![
                input.key.account_id.to_string(),
                input.key.provider_mailbox_id,
                input.key.provider_message_id,
                input.provider_read,
                input
                    .provider_revision
                    .as_ref()
                    .map(ProviderRevision::expose),
                input.completed_at_ms,
            ],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    connection
        .execute(
            "DELETE FROM pending_read_mutations
             WHERE account_id=?1 AND provider_mailbox_id=?2 AND provider_message_id=?3
               AND intent_generation=?4 AND lease_id=?5 AND state='running'",
            params![
                input.key.account_id.to_string(),
                input.key.provider_mailbox_id,
                input.key.provider_message_id,
                i64::try_from(input.generation.get()).map_err(|_| StorageError::Constraint)?,
                input.lease_id.to_string(),
            ],
        )
        .map(|changed| changed > 0)
        .map_err(|error| StorageError::from_sql(&error))
}

fn transition_desired_read_mutation(
    connection: &Connection,
    input: &TransitionDesiredReadMutationInput,
) -> Result<bool, StorageError> {
    connection
        .execute(
            "UPDATE pending_read_mutations
             SET state=?6, attempt_count=?7, next_attempt_at_ms=?8,
                 lease_id=CASE WHEN ?10 THEN NULL ELSE lease_id END,
                 lease_expires_at_ms=CASE WHEN ?10 THEN NULL ELSE lease_expires_at_ms END,
                 safe_error_code=?9,
                 updated_at_ms=max(updated_at_ms, created_at_ms, ?11)
             WHERE account_id=?1 AND provider_mailbox_id=?2 AND provider_message_id=?3
               AND intent_generation=?4 AND lease_id=?5 AND state='running'",
            params![
                input.key.account_id.to_string(),
                input.key.provider_mailbox_id,
                input.key.provider_message_id,
                i64::try_from(input.generation.get()).map_err(|_| StorageError::Constraint)?,
                input.lease_id.to_string(),
                desired_read_state_to_str(input.state),
                input.attempt_count,
                input.next_attempt_at_ms,
                input.safe_error_code.as_ref().map(SafeErrorCode::as_str),
                input.release_lease,
                input.updated_at_ms,
            ],
        )
        .map(|changed| changed > 0)
        .map_err(|error| StorageError::from_sql(&error))
}

fn save_draft_for_offline_review(
    connection: &Connection,
    input: &OfflineDraftReviewInput,
) -> Result<OfflineDraftReviewResult, StorageError> {
    let draft = save_draft(connection, &input.draft)?;
    connection
        .execute(
            "INSERT INTO draft_send_reviews(
                draft_id, draft_revision, reason, created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, 'offline', ?3, ?3)
             ON CONFLICT(draft_id) DO UPDATE SET
                draft_revision=excluded.draft_revision, reason='offline',
                updated_at_ms=excluded.updated_at_ms",
            params![
                draft.id.to_string(),
                i64::try_from(draft.revision).map_err(|_| StorageError::Constraint)?,
                input.reviewed_at_ms
            ],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    let review = DraftSendReview {
        draft_id: draft.id,
        account_id: draft.account_id,
        draft_revision: draft.revision,
        reason: DraftSendReviewReason::Offline,
        created_at_ms: input.reviewed_at_ms,
        updated_at_ms: input.reviewed_at_ms,
    };
    Ok(OfflineDraftReviewResult { draft, review })
}

fn list_send_confirmation_required(
    connection: &Connection,
    account_id: Option<AccountId>,
) -> Result<Vec<SendConfirmationRequired>, StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT d.id, d.account_id, d.revision
             FROM draft_send_reviews r JOIN drafts d ON d.id=r.draft_id
             WHERE r.reason='offline' AND r.draft_revision=d.revision
               AND (?1 IS NULL OR d.account_id=?1)
             ORDER BY r.created_at_ms, d.id",
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    let account = account_id.map(|id| id.to_string());
    statement
        .query_map([account], |row| {
            Ok(SendConfirmationRequired {
                draft_id: parse_id_sql(&row.get::<_, String>(0)?)?,
                account_id: parse_id_sql(&row.get::<_, String>(1)?)?,
                draft_revision: u64::try_from(row.get::<_, i64>(2)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                reason: DraftSendReviewReason::Offline,
            })
        })
        .map_err(|error| StorageError::from_sql(&error))?
        .map(|row| row.map_err(|error| StorageError::from_sql(&error)))
        .collect()
}

fn consume_draft_send_review(
    connection: &Connection,
    key: DraftSendReviewKey,
) -> Result<bool, StorageError> {
    connection
        .execute(
            "DELETE FROM draft_send_reviews
             WHERE draft_id=?1 AND draft_revision=?2
               AND EXISTS(SELECT 1 FROM drafts d WHERE d.id=draft_id AND d.revision=?2)",
            params![
                key.draft_id.to_string(),
                i64::try_from(key.draft_revision).map_err(|_| StorageError::Constraint)?
            ],
        )
        .map(|changed| changed > 0)
        .map_err(|error| StorageError::from_sql(&error))
}

fn prepare_outbound_attempt(
    connection: &Connection,
    input: &PrepareOutboundAttemptInput,
) -> Result<OutboundAttempt, StorageError> {
    if input.date_rfc2822.trim().is_empty()
        || input.message.as_bytes().is_empty()
        || input.message.message_id.trim().is_empty()
        || input.message.envelope.from != input.snapshot.sender.address
        || input.message.envelope.recipients.is_empty()
    {
        return Err(StorageError::Constraint);
    }
    let draft: Option<(String, i64)> = connection
        .query_row(
            "SELECT account_id, revision FROM drafts WHERE id=?1",
            [input.draft_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))?;
    let Some((account_id, revision)) = draft else {
        return Err(StorageError::DraftRevisionConflict);
    };
    if account_id != input.account_id.to_string()
        || u64::try_from(revision).ok() != Some(input.draft_revision)
    {
        return Err(StorageError::DraftRevisionConflict);
    }
    let blocked: bool = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM outbound_attempts WHERE draft_id=?1 AND send_blocked=1
             )",
            [input.draft_id.to_string()],
            |row| row.get(0),
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    if blocked {
        return Err(StorageError::Constraint);
    }
    let recipients = serde_json::json!({
        "to": encode_draft_addresses(&input.snapshot.to),
        "cc": encode_draft_addresses(&input.snapshot.cc),
        "bcc": encode_draft_addresses(&input.snapshot.bcc),
    });
    let sender = encode_draft_address(&input.snapshot.sender);
    let envelope_recipients = serde_json::to_string(&input.message.envelope.recipients)
        .map_err(|_| StorageError::Serialization)?;
    connection
        .execute(
            "INSERT INTO outbound_attempts(
                id, account_id, draft_id, draft_revision, reply_source_message_id,
                provider_thread_id, original_provider_message_id, rfc_message_id,
                date_rfc2822, exact_mime, envelope_from, envelope_recipients_json,
                sender_json, recipients_json, subject, body_plain, state, send_blocked,
                created_at_ms, updated_at_ms
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                ?13, ?14, ?15, ?16, 'submitting', 1, ?17, ?17
             )",
            params![
                input.id.to_string(),
                input.account_id.to_string(),
                input.draft_id.to_string(),
                i64::try_from(input.draft_revision).map_err(|_| StorageError::Constraint)?,
                input.in_reply_to_message_id.map(|id| id.to_string()),
                input.provider_thread_id,
                input.original_provider_message_id,
                input.message.message_id,
                input.date_rfc2822,
                input.message.as_bytes(),
                input.message.envelope.from,
                envelope_recipients,
                sender.to_string(),
                recipients.to_string(),
                input.snapshot.subject,
                input.snapshot.plain_body,
                input.created_at_ms,
            ],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    get_outbound_attempt(connection, input.id)?.ok_or(StorageError::Serialization)
}

fn complete_outbound_attempt(
    connection: &Connection,
    input: &CompleteOutboundAttemptInput,
) -> Result<OutboundAttempt, StorageError> {
    let attempt: Option<(String, String, i64, String)> = connection
        .query_row(
            "SELECT draft_id, account_id, draft_revision, state
             FROM outbound_attempts WHERE id=?1",
            [input.attempt_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))?;
    let Some((draft_id, account_id, draft_revision, state)) = attempt else {
        return Err(StorageError::NotFound);
    };
    if state != "submitting" {
        return Err(StorageError::Constraint);
    }
    let (next_state, send_blocked, provider_message_id, safe_error_code) = match &input.outcome {
        OutboundAttemptOutcome::Accepted {
            provider_message_id,
        } => (
            "accepted_pending",
            false,
            provider_message_id.as_deref(),
            None,
        ),
        OutboundAttemptOutcome::Rejected { safe_error_code } => (
            "rejected",
            false,
            None,
            Some(outbound_failure_code_to_str(*safe_error_code)),
        ),
        OutboundAttemptOutcome::UnknownAfterSubmission => ("unknown_locked", true, None, None),
    };
    let changed = connection
        .execute(
            "UPDATE outbound_attempts
             SET state=?2, send_blocked=?3, provider_message_id=?4,
                 safe_error_code=?5, updated_at_ms=max(updated_at_ms, created_at_ms, ?6)
             WHERE id=?1 AND state='submitting'",
            params![
                input.attempt_id.to_string(),
                next_state,
                send_blocked,
                provider_message_id,
                safe_error_code,
                input.updated_at_ms,
            ],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    if changed != 1 {
        return Err(StorageError::Constraint);
    }
    if matches!(&input.outcome, OutboundAttemptOutcome::Accepted { .. }) {
        connection
            .execute(
                "DELETE FROM drafts WHERE id=?1 AND account_id=?2 AND revision=?3",
                params![draft_id, account_id, draft_revision],
            )
            .map_err(|error| StorageError::from_sql(&error))?;
    }
    get_outbound_attempt(connection, input.attempt_id)?.ok_or(StorageError::Serialization)
}

fn reconcile_outbound_attempt(
    connection: &Connection,
    input: &ReconcileOutboundAttemptInput,
) -> Result<OutboundAttempt, StorageError> {
    let attempt =
        get_outbound_attempt(connection, input.attempt_id)?.ok_or(StorageError::NotFound)?;
    if attempt.state == OutboundAttemptState::Reconciled {
        return Ok(attempt);
    }
    if !matches!(
        attempt.state,
        OutboundAttemptState::AcceptedPending | OutboundAttemptState::UnknownLocked
    ) || input.mailbox.role != MailboxRole::Sent
        || input.mailbox.key.account_id != attempt.account_id
        || input.message.key.account_id != attempt.account_id
        || input.message.key.provider_mailbox_id != input.mailbox.key.provider_mailbox_id
        || normalized_message_id(input.message.mime.message_id.as_deref())
            != normalized_message_id(Some(&attempt.message.message_id))
    {
        return Err(StorageError::Constraint);
    }
    upsert_remote_mailbox(connection, &input.mailbox, input.reconciled_at_ms)?;
    let (_, _, message_id) = upsert_remote_message(
        connection,
        &input.message,
        MessageDirection::Outgoing,
        input.reconciled_at_ms,
    )?;
    let changed = connection
        .execute(
            "UPDATE outbound_attempts
             SET state='reconciled', send_blocked=0, retry_authorized=0,
                 provider_message_id=COALESCE(provider_message_id, ?2),
                 reconciled_message_id=?3, safe_error_code=NULL,
                 updated_at_ms=max(updated_at_ms, created_at_ms, ?4)
             WHERE id=?1 AND state IN ('accepted_pending', 'unknown_locked')",
            params![
                input.attempt_id.to_string(),
                input.message.key.provider_message_id,
                message_id.to_string(),
                input.reconciled_at_ms,
            ],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    if changed != 1 {
        return Err(StorageError::Constraint);
    }
    connection
        .execute(
            "DELETE FROM drafts WHERE id=?1 AND account_id=?2 AND revision=?3",
            params![
                attempt.draft_id.to_string(),
                attempt.account_id.to_string(),
                i64::try_from(attempt.draft_revision).map_err(|_| StorageError::Serialization)?,
            ],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    get_outbound_attempt(connection, input.attempt_id)?.ok_or(StorageError::Serialization)
}

fn normalized_message_id(value: Option<&str>) -> Option<&str> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.strip_prefix('<').unwrap_or(value))
        .map(|value| value.strip_suffix('>').unwrap_or(value))
}

#[derive(Debug)]
struct OutboundAttemptRow {
    id: String,
    account_id: String,
    draft_id: String,
    draft_revision: i64,
    reply_source_message_id: Option<String>,
    provider_thread_id: Option<String>,
    original_provider_message_id: Option<String>,
    rfc_message_id: String,
    date_rfc2822: String,
    exact_mime: Vec<u8>,
    envelope_from: String,
    envelope_recipients_json: String,
    sender_json: String,
    recipients_json: String,
    subject: String,
    body_plain: String,
    state: String,
    provider_message_id: Option<String>,
    reconciled_message_id: Option<String>,
    safe_error_code: Option<String>,
    sent_refresh_count: i64,
    retry_authorized: bool,
    created_at_ms: i64,
    updated_at_ms: i64,
}

fn read_outbound_attempt_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OutboundAttemptRow> {
    Ok(OutboundAttemptRow {
        id: row.get(0)?,
        account_id: row.get(1)?,
        draft_id: row.get(2)?,
        draft_revision: row.get(3)?,
        reply_source_message_id: row.get(4)?,
        provider_thread_id: row.get(5)?,
        original_provider_message_id: row.get(6)?,
        rfc_message_id: row.get(7)?,
        date_rfc2822: row.get(8)?,
        exact_mime: row.get(9)?,
        envelope_from: row.get(10)?,
        envelope_recipients_json: row.get(11)?,
        sender_json: row.get(12)?,
        recipients_json: row.get(13)?,
        subject: row.get(14)?,
        body_plain: row.get(15)?,
        state: row.get(16)?,
        provider_message_id: row.get(17)?,
        reconciled_message_id: row.get(18)?,
        safe_error_code: row.get(19)?,
        sent_refresh_count: row.get(20)?,
        retry_authorized: row.get(21)?,
        created_at_ms: row.get(22)?,
        updated_at_ms: row.get(23)?,
    })
}

const OUTBOUND_ATTEMPT_COLUMNS: &str =
    "id, account_id, draft_id, draft_revision, reply_source_message_id,
     provider_thread_id, original_provider_message_id, rfc_message_id, date_rfc2822,
     exact_mime, envelope_from, envelope_recipients_json, sender_json, recipients_json,
     subject, body_plain, state, provider_message_id, reconciled_message_id,
     safe_error_code, sent_refresh_count, retry_authorized, created_at_ms, updated_at_ms";

fn get_outbound_attempt(
    connection: &Connection,
    attempt_id: OutboundAttemptId,
) -> Result<Option<OutboundAttempt>, StorageError> {
    connection
        .query_row(
            &format!("SELECT {OUTBOUND_ATTEMPT_COLUMNS} FROM outbound_attempts WHERE id=?1"),
            [attempt_id.to_string()],
            read_outbound_attempt_row,
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))?
        .map(outbound_attempt_from_row)
        .transpose()
}

fn outbound_attempt_from_row(row: OutboundAttemptRow) -> Result<OutboundAttempt, StorageError> {
    let envelope_recipients: Vec<String> = serde_json::from_str(&row.envelope_recipients_json)
        .map_err(|_| StorageError::Serialization)?;
    if envelope_recipients
        .iter()
        .any(|value| value.trim().is_empty())
    {
        return Err(StorageError::Serialization);
    }
    let sender_value: serde_json::Value =
        serde_json::from_str(&row.sender_json).map_err(|_| StorageError::Serialization)?;
    let recipients: serde_json::Value =
        serde_json::from_str(&row.recipients_json).map_err(|_| StorageError::Serialization)?;
    let sender = decode_draft_address(&sender_value)?;
    let addresses = |name: &str| decode_draft_addresses(recipients.get(name));
    let rfc_message_id = row.rfc_message_id;
    Ok(OutboundAttempt {
        id: parse_id(&row.id)?,
        draft_id: parse_id(&row.draft_id)?,
        draft_revision: u64::try_from(row.draft_revision)
            .map_err(|_| StorageError::Serialization)?,
        account_id: parse_id(&row.account_id)?,
        in_reply_to_message_id: row
            .reply_source_message_id
            .map(|value| parse_id(&value))
            .transpose()?,
        provider_thread_id: row.provider_thread_id,
        original_provider_message_id: row.original_provider_message_id,
        date_rfc2822: row.date_rfc2822,
        message: ComposedMessage::new(
            row.exact_mime,
            rfc_message_id,
            DeliveryEnvelope {
                from: row.envelope_from,
                recipients: envelope_recipients,
            },
        ),
        snapshot: OutboundAttemptSnapshot {
            sender,
            to: addresses("to")?,
            cc: addresses("cc")?,
            bcc: addresses("bcc")?,
            subject: row.subject,
            plain_body: row.body_plain,
        },
        state: outbound_attempt_state_from_str(&row.state)?,
        provider_message_id: row.provider_message_id,
        reconciled_message_id: row
            .reconciled_message_id
            .map(|value| parse_id(&value))
            .transpose()?,
        safe_error_code: row
            .safe_error_code
            .as_deref()
            .map(outbound_failure_code_from_str)
            .transpose()?,
        sent_refresh_count: u32::try_from(row.sent_refresh_count)
            .map_err(|_| StorageError::Serialization)?,
        retry_authorized: row.retry_authorized,
        created_at_ms: row.created_at_ms,
        updated_at_ms: row.updated_at_ms,
    })
}

fn list_sent_projections(
    connection: &Connection,
    account_id: Option<AccountId>,
) -> Result<Vec<SentProjection>, StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT id FROM outbound_attempts
             WHERE state IN ('accepted_pending', 'reconciled', 'unknown_locked')
               AND (?1 IS NULL OR account_id=?1)
             ORDER BY updated_at_ms DESC, id DESC",
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    let account = account_id.map(|id| id.to_string());
    let ids = statement
        .query_map([account], |row| row.get::<_, String>(0))
        .map_err(|error| StorageError::from_sql(&error))?
        .map(|row| {
            row.map_err(|error| StorageError::from_sql(&error))
                .and_then(|value| parse_id(&value))
        })
        .collect::<Result<Vec<OutboundAttemptId>, StorageError>>()?;
    ids.into_iter()
        .map(|id| {
            get_outbound_attempt(connection, id)?
                .map(|attempt| SentProjection { attempt })
                .ok_or(StorageError::Serialization)
        })
        .collect()
}

fn record_sent_refresh(
    connection: &Connection,
    input: RecordSentRefreshInput,
) -> Result<u32, StorageError> {
    connection
        .execute(
            "UPDATE outbound_attempts
             SET sent_refresh_count=sent_refresh_count + 1,
                 updated_at_ms=max(updated_at_ms, created_at_ms, ?2)
             WHERE account_id=?1 AND state IN ('accepted_pending', 'unknown_locked')",
            params![input.account_id.to_string(), input.refreshed_at_ms],
        )
        .map_err(|error| StorageError::from_sql(&error))
        .and_then(|count| u32::try_from(count).map_err(|_| StorageError::Serialization))
}

fn authorize_outbound_retry(
    connection: &Connection,
    input: AuthorizeOutboundRetryInput,
) -> Result<bool, StorageError> {
    connection
        .execute(
            "UPDATE outbound_attempts
             SET retry_authorized=1, send_blocked=0,
                 updated_at_ms=max(updated_at_ms, created_at_ms, ?2)
             WHERE id=?1 AND state='unknown_locked' AND send_blocked=1
               AND retry_authorized=0 AND sent_refresh_count>=1",
            params![input.attempt_id.to_string(), input.authorized_at_ms],
        )
        .map(|changed| changed > 0)
        .map_err(|error| StorageError::from_sql(&error))
}

fn recover_submitting_outbound_attempts(
    connection: &Connection,
    recovered_at_ms: i64,
) -> Result<u32, StorageError> {
    connection
        .execute(
            "UPDATE outbound_attempts
             SET state='unknown_locked', send_blocked=1, retry_authorized=0,
                 updated_at_ms=max(updated_at_ms, created_at_ms, ?1)
             WHERE state='submitting'",
            [recovered_at_ms],
        )
        .map_err(|error| StorageError::from_sql(&error))
        .and_then(|count| u32::try_from(count).map_err(|_| StorageError::Serialization))
}

fn encode_draft_address(address: &DraftAddress) -> serde_json::Value {
    serde_json::json!({
        "displayName": address.display_name,
        "address": address.address,
    })
}

fn decode_draft_address(value: &serde_json::Value) -> Result<DraftAddress, StorageError> {
    let object = value.as_object().ok_or(StorageError::Serialization)?;
    let display_name = object
        .get("displayName")
        .filter(|value| !value.is_null())
        .map(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .ok_or(StorageError::Serialization)
        })
        .transpose()?;
    let address = object
        .get("address")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or(StorageError::Serialization)?
        .to_owned();
    Ok(DraftAddress {
        display_name,
        address,
    })
}

fn outbound_attempt_state_from_str(value: &str) -> Result<OutboundAttemptState, StorageError> {
    match value {
        "submitting" => Ok(OutboundAttemptState::Submitting),
        "accepted_pending" => Ok(OutboundAttemptState::AcceptedPending),
        "reconciled" => Ok(OutboundAttemptState::Reconciled),
        "rejected" => Ok(OutboundAttemptState::Rejected),
        "unknown_locked" => Ok(OutboundAttemptState::UnknownLocked),
        _ => Err(StorageError::Serialization),
    }
}

const fn outbound_failure_code_to_str(value: OutboundFailureCode) -> &'static str {
    match value {
        OutboundFailureCode::RecipientRejected => "recipient_rejected",
        OutboundFailureCode::AuthenticationRequired => "authentication_required",
        OutboundFailureCode::ProviderUnavailable => "provider_unavailable",
        OutboundFailureCode::InvalidDraft => "invalid_draft",
        OutboundFailureCode::Internal => "internal",
    }
}

fn outbound_failure_code_from_str(value: &str) -> Result<OutboundFailureCode, StorageError> {
    match value {
        "recipient_rejected" => Ok(OutboundFailureCode::RecipientRejected),
        "authentication_required" => Ok(OutboundFailureCode::AuthenticationRequired),
        "provider_unavailable" => Ok(OutboundFailureCode::ProviderUnavailable),
        "invalid_draft" => Ok(OutboundFailureCode::InvalidDraft),
        "internal" => Ok(OutboundFailureCode::Internal),
        _ => Err(StorageError::Serialization),
    }
}

fn schedule_sync_operation(
    connection: &Connection,
    input: &ScheduleSyncInput,
) -> Result<SyncOperation, StorageError> {
    let active: Option<String> = connection
        .query_row(
            "SELECT id FROM sync_operations
             WHERE account_id=?1 AND scope=?2
               AND state IN ('scheduled', 'running', 'waiting_backoff', 'offline', 'needs_auth')",
            params![input.account_id.to_string(), input.scope],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))?;
    let operation_id = if let Some(id) = active {
        let id: OperationId = parse_id(&id)?;
        let restore_offline = matches!(input.trigger, SyncTrigger::ConnectivityRestored);
        connection
            .execute(
                "UPDATE sync_operations
                 SET trigger_bits=(trigger_bits | ?2),
                     state=CASE WHEN state='offline' AND ?4 THEN 'scheduled' ELSE state END,
                     next_attempt_at_ms=CASE WHEN state='offline' AND ?4 THEN NULL ELSE next_attempt_at_ms END,
                     updated_at_ms=max(updated_at_ms, created_at_ms, ?3)
                 WHERE id=?1",
                params![
                    id.to_string(),
                    input.trigger.bit(),
                    input.scheduled_at_ms,
                    restore_offline
                ],
            )
            .map_err(|error| StorageError::from_sql(&error))?;
        id
    } else {
        let (mode, mode_limit) = sync_mode_to_db(input.mode);
        connection
            .execute(
                "INSERT INTO sync_operations(
                    id, account_id, scope, trigger_bits, mode, mode_limit, stage, state,
                    attempt_count, cancel_generation, scheduled_at_ms, updated_at_ms, created_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, 'scheduled', 0, 0, ?7, ?7, ?7)",
                params![
                    input.operation_id.to_string(),
                    input.account_id.to_string(),
                    input.scope,
                    input.trigger.bit(),
                    mode,
                    mode_limit,
                    input.scheduled_at_ms,
                ],
            )
            .map_err(|error| StorageError::from_sql(&error))?;
        input.operation_id
    };
    load_sync_operation(connection, operation_id)?.ok_or(StorageError::Constraint)
}

fn load_sync_operation(
    connection: &Connection,
    operation_id: OperationId,
) -> Result<Option<SyncOperation>, StorageError> {
    connection
        .query_row(
            "SELECT sync_operations.id, sync_operations.account_id, sync_operations.scope,
                    sync_operations.trigger_bits, sync_operations.mode,
                    sync_operations.mode_limit, sync_operations.stage, sync_operations.state,
                    sync_operations.attempt_count, sync_operations.next_attempt_at_ms,
                    sync_operations.lease_id, sync_operations.lease_expires_at_ms,
                    sync_operations.cancel_generation, sync_operations.safe_error_code,
                    sync_operations.created_at_ms, sync_operations.updated_at_ms,
                    sync_operations.started_at_ms, sync_operations.finished_at_ms
             FROM sync_operations WHERE id=?1",
            [operation_id.to_string()],
            sync_operation_from_row,
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))
}

fn sync_operation_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SyncOperation> {
    let mode_raw = row.get::<_, String>(4)?;
    let mode_limit = row.get::<_, Option<i64>>(5)?;
    let mode = sync_mode_from_db(&mode_raw, mode_limit).map_err(storage_to_sql)?;
    let status_name = row.get::<_, String>(7)?;
    let phase_name = row.get::<_, Option<String>>(6)?;
    let lifecycle =
        sync_state_from_db(&status_name, phase_name.as_deref()).map_err(storage_to_sql)?;
    let trigger_bits = u8::try_from(row.get::<_, i64>(3)?)
        .ok()
        .and_then(SyncTriggerSet::from_bits)
        .ok_or(rusqlite::Error::InvalidQuery)?;
    let lease = match (
        row.get::<_, Option<String>>(10)?,
        row.get::<_, Option<i64>>(11)?,
    ) {
        (Some(id), Some(expires_at_ms)) => Some(OperationLease {
            id: parse_id_sql(&id)?,
            expires_at_ms,
        }),
        (None, None) => None,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    let safe_error_code = row
        .get::<_, Option<String>>(13)?
        .map(|value| SafeErrorCode::new(value).ok_or(rusqlite::Error::InvalidQuery))
        .transpose()?;
    Ok(SyncOperation {
        id: parse_id_sql(&row.get::<_, String>(0)?)?,
        account_id: parse_id_sql(&row.get::<_, String>(1)?)?,
        scope: row.get(2)?,
        triggers: trigger_bits,
        mode,
        state: lifecycle,
        attempt_count: u32::try_from(row.get::<_, i64>(8)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        next_attempt_at_ms: row.get(9)?,
        lease,
        cancel_generation: u64::try_from(row.get::<_, i64>(12)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        safe_error_code,
        created_at_ms: row.get(14)?,
        updated_at_ms: row.get(15)?,
        started_at_ms: row.get(16)?,
        finished_at_ms: row.get(17)?,
    })
}

fn sync_summary(operation: SyncOperation) -> SyncOperationSummary {
    SyncOperationSummary {
        operation_id: operation.id,
        account_id: operation.account_id,
        state: operation.state,
        triggers: operation.triggers,
        attempt_count: operation.attempt_count,
        next_attempt_at_ms: operation.next_attempt_at_ms,
        safe_error_code: operation.safe_error_code,
        created_at_ms: operation.created_at_ms,
        updated_at_ms: operation.updated_at_ms,
        finished_at_ms: operation.finished_at_ms,
    }
}

fn list_runnable_sync_operations(
    connection: &Connection,
    provider: Provider,
    now_ms: i64,
    limit: u32,
) -> Result<Vec<SyncOperationSummary>, StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT operations.id, operations.account_id, operations.scope,
                    operations.trigger_bits, operations.mode, operations.mode_limit,
                    operations.stage, operations.state, operations.attempt_count,
                    operations.next_attempt_at_ms, operations.lease_id,
                    operations.lease_expires_at_ms, operations.cancel_generation,
                    operations.safe_error_code, operations.created_at_ms,
                    operations.updated_at_ms, operations.started_at_ms,
                    operations.finished_at_ms
             FROM sync_operations AS operations
             INNER JOIN accounts AS account ON account.id=operations.account_id
             WHERE operations.state IN ('scheduled', 'waiting_backoff')
               AND account.provider=?1 AND account.deleting=0 AND account.enabled=1
               AND (operations.next_attempt_at_ms IS NULL
                    OR operations.next_attempt_at_ms<=?2 OR ?2<operations.updated_at_ms)
               AND (operations.lease_expires_at_ms IS NULL
                    OR operations.lease_expires_at_ms<=?2 OR ?2<operations.updated_at_ms)
             ORDER BY coalesce(operations.next_attempt_at_ms, operations.scheduled_at_ms),
                      operations.scheduled_at_ms, operations.id
             LIMIT ?3",
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    statement
        .query_map(
            params![
                provider_to_str(provider),
                now_ms,
                i64::from(limit.clamp(1, 100))
            ],
            |row| sync_operation_from_row(row).map(sync_summary),
        )
        .map_err(|error| StorageError::from_sql(&error))?
        .map(|row| row.map_err(|error| StorageError::from_sql(&error)))
        .collect()
}

fn clamp_durable_time(observed_at_ms: i64, created_at_ms: i64, updated_at_ms: i64) -> i64 {
    observed_at_ms.max(0).max(created_at_ms).max(updated_at_ms)
}

fn clamp_lease(lease: OperationLease, observed_at_ms: i64, durable_at_ms: i64) -> OperationLease {
    let duration_ms = lease.expires_at_ms.saturating_sub(observed_at_ms).max(0);
    OperationLease {
        id: lease.id,
        expires_at_ms: durable_at_ms.saturating_add(duration_ms),
    }
}

fn claim_sync_operation(
    connection: &Connection,
    input: ClaimSyncOperationInput,
) -> Result<Option<SyncOperation>, StorageError> {
    let account_provider: Option<String> = connection
        .query_row(
            "SELECT provider FROM accounts WHERE id=(
                SELECT account_id FROM sync_operations WHERE id=?1
             ) AND deleting=0 AND enabled=1",
            [input.operation_id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))?;
    if account_provider.as_deref() != Some(provider_to_str(input.provider)) {
        return Ok(None);
    }
    let Some(mut operation) = load_sync_operation(connection, input.operation_id)? else {
        return Ok(None);
    };
    if !matches!(
        operation.state,
        SyncState::Scheduled | SyncState::WaitingBackoff
    ) || operation.next_attempt_at_ms.is_some_and(|deadline| {
        deadline > input.claimed_at_ms && input.claimed_at_ms >= operation.updated_at_ms
    }) || operation.lease.is_some_and(|lease| {
        lease.expires_at_ms > input.claimed_at_ms && input.claimed_at_ms >= operation.updated_at_ms
    }) {
        return Ok(None);
    }
    let claimed_at_ms = clamp_durable_time(
        input.claimed_at_ms,
        operation.created_at_ms,
        operation.updated_at_ms,
    );
    let lease = clamp_lease(input.lease, input.claimed_at_ms, claimed_at_ms);
    let changed = connection
        .execute(
            "UPDATE sync_operations
             SET state='running', stage='load', lease_id=?2, lease_expires_at_ms=?3,
                 next_attempt_at_ms=NULL, started_at_ms=coalesce(started_at_ms, ?4),
                 updated_at_ms=?4, trigger_bits=0
             WHERE id=?1 AND state IN ('scheduled', 'waiting_backoff')
               AND (next_attempt_at_ms IS NULL OR next_attempt_at_ms<=?5 OR ?5<updated_at_ms)
               AND (lease_expires_at_ms IS NULL OR lease_expires_at_ms<=?5 OR ?5<updated_at_ms)
               AND NOT EXISTS(
                   SELECT 1 FROM sync_operations competing
                   WHERE competing.account_id=sync_operations.account_id
                     AND competing.id<>sync_operations.id AND competing.state='running'
               )",
            params![
                input.operation_id.to_string(),
                lease.id.to_string(),
                lease.expires_at_ms,
                claimed_at_ms,
                input.claimed_at_ms,
            ],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    if changed == 0 {
        return Ok(None);
    }
    operation.state = SyncState::Running(SyncStage::Load);
    operation.next_attempt_at_ms = None;
    operation.lease = Some(lease);
    operation.started_at_ms = operation.started_at_ms.or(Some(claimed_at_ms));
    operation.updated_at_ms = claimed_at_ms;
    Ok(Some(operation))
}

fn transition_sync_operation(
    connection: &Connection,
    input: &TransitionSyncOperationInput,
) -> Result<bool, StorageError> {
    let (status_value, phase_value) = sync_state_to_db(input.state);
    let terminal = matches!(
        input.state,
        SyncState::Committed | SyncState::Failed | SyncState::Cancelled
    );
    let (mode, mode_limit) = input
        .mode
        .map(sync_mode_to_db)
        .map_or((None, None), |(mode, limit)| (Some(mode), limit));
    connection
        .execute(
            "UPDATE sync_operations
             SET state=?3, stage=?4, attempt_count=?5, next_attempt_at_ms=?6,
                 safe_error_code=?7,
                 updated_at_ms=max(updated_at_ms, created_at_ms, ?8),
                 finished_at_ms=CASE WHEN ?9 THEN max(updated_at_ms, created_at_ms, ?8) ELSE NULL END,
                 lease_id=CASE WHEN ?10 THEN lease_id ELSE NULL END,
                 lease_expires_at_ms=CASE WHEN ?10 THEN lease_expires_at_ms ELSE NULL END,
                 mode=coalesce(?11, mode), mode_limit=CASE WHEN ?11 IS NULL THEN mode_limit ELSE ?12 END
             WHERE id=?1 AND lease_id=?2 AND state='running'",
            params![
                input.operation_id.to_string(), input.lease_id.to_string(), status_value,
                phase_value,
                input.attempt_count, input.next_attempt_at_ms,
                input.safe_error_code.as_ref().map(SafeErrorCode::as_str), input.updated_at_ms,
                terminal, matches!(input.state, SyncState::Running(_)), mode, mode_limit,
            ],
        )
        .map(|changed| changed > 0)
        .map_err(|error| StorageError::from_sql(&error))
}

fn get_sync_operation_summary(
    connection: &Connection,
    operation_id: OperationId,
) -> Result<Option<SyncOperationSummary>, StorageError> {
    load_sync_operation(connection, operation_id).map(|value| value.map(sync_summary))
}

fn list_sync_operations(
    connection: &Connection,
    account_id: AccountId,
    limit: u32,
) -> Result<Vec<SyncOperationSummary>, StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT id, account_id, scope, trigger_bits, mode, mode_limit, stage, state,
                    attempt_count, next_attempt_at_ms, lease_id, lease_expires_at_ms,
                    cancel_generation, safe_error_code, created_at_ms, updated_at_ms,
                    started_at_ms, finished_at_ms
             FROM sync_operations WHERE account_id=?1
             ORDER BY created_at_ms DESC, id DESC LIMIT ?2",
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    statement
        .query_map(
            params![account_id.to_string(), i64::from(limit.clamp(1, 100))],
            |row| sync_operation_from_row(row).map(sync_summary),
        )
        .map_err(|error| StorageError::from_sql(&error))?
        .map(|row| row.map_err(|error| StorageError::from_sql(&error)))
        .collect()
}

fn recover_expired_leases(
    connection: &Connection,
    now_ms: i64,
) -> Result<LeaseRecoveryResult, StorageError> {
    let sync_operations_recovered = connection
        .execute(
            "UPDATE sync_operations
             SET state='scheduled', stage=NULL, lease_id=NULL, lease_expires_at_ms=NULL,
                 next_attempt_at_ms=NULL,
                 updated_at_ms=max(updated_at_ms, created_at_ms, ?1)
             WHERE state='running' AND (lease_expires_at_ms<=?1 OR ?1<updated_at_ms)",
            [now_ms],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    let read_mutations_recovered = connection
        .execute(
            "UPDATE pending_read_mutations
             SET state='pending', lease_id=NULL, lease_expires_at_ms=NULL,
                 next_attempt_at_ms=NULL,
                 updated_at_ms=max(updated_at_ms, created_at_ms, ?1)
             WHERE state='running' AND (lease_expires_at_ms<=?1 OR ?1<updated_at_ms)",
            [now_ms],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    Ok(LeaseRecoveryResult {
        sync_operations_recovered: u32::try_from(sync_operations_recovered)
            .map_err(|_| StorageError::Serialization)?,
        read_mutations_recovered: u32::try_from(read_mutations_recovered)
            .map_err(|_| StorageError::Serialization)?,
    })
}

#[allow(clippy::too_many_lines)]
fn commit_sync_batch(
    connection: &Connection,
    input: &SyncBatchInput,
) -> Result<SyncBatchResult, StorageError> {
    let operation: Option<(String, String, i64, i64)> = connection
        .query_row(
            "SELECT account_id, scope, created_at_ms, updated_at_ms FROM sync_operations
             WHERE id=?1 AND lease_id=?2 AND state='running'",
            params![input.operation_id.to_string(), input.lease_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))?;
    if operation
        .as_ref()
        .map(|(account, scope, _, _)| (account.as_str(), scope.as_str()))
        != Some((
            input.cursor_key.account_id.to_string().as_str(),
            input.cursor_key.scope.as_str(),
        ))
    {
        return Err(StorageError::Constraint);
    }
    let (_, _, operation_created_at_ms, operation_updated_at_ms) =
        operation.ok_or(StorageError::Constraint)?;
    let committed_at_ms = clamp_durable_time(
        input.committed_at_ms,
        operation_created_at_ms,
        operation_updated_at_ms,
    );
    for mailbox in &input.mailboxes {
        if mailbox.key.account_id != input.cursor_key.account_id {
            return Err(StorageError::Constraint);
        }
        upsert_remote_mailbox(connection, mailbox, committed_at_ms)?;
    }
    let mut result = SyncBatchResult {
        operation_id: input.operation_id,
        inserted_messages: 0,
        updated_messages: 0,
        removed_messages: 0,
        acknowledged_read_mutations: 0,
    };
    for change in &input.changes {
        match change {
            RemoteChange::Upsert(message) => {
                if message.key.account_id != input.cursor_key.account_id {
                    return Err(StorageError::Constraint);
                }
                let (inserted, acknowledged, _) = upsert_remote_message(
                    connection,
                    message,
                    MessageDirection::Incoming,
                    committed_at_ms,
                )?;
                if inserted {
                    result.inserted_messages += 1;
                } else {
                    result.updated_messages += 1;
                }
                result.acknowledged_read_mutations += u32::from(acknowledged);
            }
            RemoteChange::ReadState {
                key,
                read,
                revision,
            } => {
                if key.account_id != input.cursor_key.account_id {
                    return Err(StorageError::Constraint);
                }
                result.acknowledged_read_mutations += u32::from(apply_remote_read_state(
                    connection,
                    key,
                    *read,
                    revision.as_ref(),
                    committed_at_ms,
                )?);
            }
            RemoteChange::Gone(key) => {
                if key.account_id != input.cursor_key.account_id {
                    return Err(StorageError::Constraint);
                }
                result.removed_messages += u32::from(apply_remote_gone(connection, key)?);
            }
        }
    }
    store_sync_cursor(
        connection,
        &SyncCursor {
            key: input.cursor_key.clone(),
            checkpoint: input.checkpoint.clone(),
            updated_at_ms: committed_at_ms,
            last_successful_sync_at_ms: Some(committed_at_ms),
        },
    )?;
    let changed = connection
        .execute(
            "UPDATE sync_operations
             SET state=CASE WHEN trigger_bits=0 THEN 'committed' ELSE 'scheduled' END,
                 stage=NULL, cursor_after_json=?3,
                 lease_id=NULL, lease_expires_at_ms=NULL, safe_error_code=NULL,
                 mode=CASE WHEN trigger_bits=0 THEN mode ELSE 'incremental' END,
                 mode_limit=CASE WHEN trigger_bits=0 THEN mode_limit ELSE NULL END,
                 attempt_count=CASE WHEN trigger_bits=0 THEN attempt_count ELSE 0 END,
                 next_attempt_at_ms=NULL, updated_at_ms=?4,
                 finished_at_ms=CASE WHEN trigger_bits=0 THEN ?4 ELSE NULL END,
                 scheduled_at_ms=CASE WHEN trigger_bits=0 THEN scheduled_at_ms ELSE ?4 END
             WHERE id=?1 AND lease_id=?2 AND state='running'",
            params![
                input.operation_id.to_string(),
                input.lease_id.to_string(),
                input.checkpoint.cursor().as_json(),
                committed_at_ms,
            ],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    if changed != 1 {
        return Err(StorageError::Constraint);
    }
    Ok(result)
}

fn upsert_remote_mailbox(
    connection: &Connection,
    mailbox: &RemoteMailbox,
    updated_at_ms: i64,
) -> Result<Mailbox, StorageError> {
    upsert_mailbox(
        connection,
        &MailboxUpsertInput {
            id: MailboxId::new(),
            account_id: mailbox.key.account_id,
            provider_mailbox_id: mailbox.key.provider_mailbox_id.clone(),
            role: mailbox.role,
            display_name: mailbox.display_name.clone(),
            updated_at_ms,
        },
    )
}

fn resolve_remote_message(
    connection: &Connection,
    key: &RemoteMessageKey,
    created_at_ms: i64,
) -> Result<(MessageId, MailboxId), StorageError> {
    let mailbox_id: Option<String> = connection
        .query_row(
            "SELECT id FROM mailboxes WHERE account_id=?1 AND provider_mailbox_id=?2",
            params![key.account_id.to_string(), key.provider_mailbox_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))?;
    let mailbox_id = mailbox_id.ok_or(StorageError::Constraint)?;
    let mapping: Option<String> = connection
        .query_row(
            "SELECT message_id FROM remote_message_ids
             WHERE account_id=?1 AND provider_mailbox_id=?2 AND provider_message_id=?3",
            params![
                key.account_id.to_string(),
                key.provider_mailbox_id,
                key.provider_message_id,
            ],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))?;
    let message_id = mapping
        .map(|value| parse_id(&value))
        .transpose()?
        .unwrap_or_else(MessageId::new);
    connection
        .execute(
            "INSERT INTO remote_message_ids(
                account_id, provider_mailbox_id, provider_message_id, message_id, created_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(account_id, provider_mailbox_id, provider_message_id) DO NOTHING",
            params![
                key.account_id.to_string(),
                key.provider_mailbox_id,
                key.provider_message_id,
                message_id.to_string(),
                created_at_ms,
            ],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    Ok((message_id, parse_id(&mailbox_id)?))
}

#[allow(clippy::too_many_lines)]
fn upsert_remote_message(
    connection: &Connection,
    message: &RemoteMessage,
    direction: MessageDirection,
    updated_at_ms: i64,
) -> Result<(bool, bool, MessageId), StorageError> {
    let (message_id, mailbox_id) = resolve_remote_message(connection, &message.key, updated_at_ms)?;
    let existing: Option<(i64, i64)> = connection
        .query_row(
            "SELECT created_at_ms, updated_at_ms FROM messages WHERE id=?1",
            [message_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))?;
    let inserted = existing.is_none();
    let pending_desired: Option<bool> = connection
        .query_row(
            "SELECT desired_read FROM pending_read_mutations WHERE message_id=?1",
            [message_id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))?;
    let effective_read = pending_desired.unwrap_or(message.read);
    let references_json =
        serde_json::to_string(&message.mime.references).map_err(|_| StorageError::Serialization)?;
    let snippet = message
        .mime
        .body
        .plain
        .as_deref()
        .unwrap_or_default()
        .chars()
        .take(240)
        .collect::<String>();
    let (created_at_ms, current_updated_at_ms) =
        existing.unwrap_or((updated_at_ms.max(0), updated_at_ms.max(0)));
    let updated_at_ms = clamp_durable_time(updated_at_ms, created_at_ms, current_updated_at_ms);
    connection
        .execute(
            "INSERT INTO messages(
                id, account_id, mailbox_id, provider_message_id, provider_revision,
                thread_id, rfc_message_id, in_reply_to, references_json, subject, snippet,
                body_plain, body_html, remote_is_read, is_read, direction, sent_at_ms,
                received_at_ms, parser_version, sanitizer_version, created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                       ?13, ?14, ?15, ?16, ?17, ?18, 1, 1, ?19, ?20)
             ON CONFLICT(id) DO UPDATE SET
                mailbox_id=excluded.mailbox_id, provider_revision=excluded.provider_revision,
                thread_id=excluded.thread_id, rfc_message_id=excluded.rfc_message_id,
                in_reply_to=excluded.in_reply_to, references_json=excluded.references_json,
                subject=excluded.subject, snippet=excluded.snippet,
                body_plain=excluded.body_plain, body_html=excluded.body_html,
                remote_is_read=excluded.remote_is_read, is_read=excluded.is_read,
                sent_at_ms=excluded.sent_at_ms, received_at_ms=excluded.received_at_ms,
                updated_at_ms=max(messages.updated_at_ms, excluded.updated_at_ms)",
            params![
                message_id.to_string(),
                message.key.account_id.to_string(),
                mailbox_id.to_string(),
                message.key.provider_message_id,
                message
                    .provider_revision
                    .as_ref()
                    .map(ProviderRevision::expose),
                message.provider_thread_id,
                message.mime.message_id,
                message.mime.in_reply_to,
                references_json,
                message.mime.subject.as_deref().unwrap_or_default(),
                snippet,
                message.mime.body.plain,
                message.mime.body.html,
                message.read,
                effective_read,
                match direction {
                    MessageDirection::Incoming => "incoming",
                    MessageDirection::Outgoing => "outgoing",
                },
                message.sent_at_ms,
                message.received_at_ms,
                created_at_ms,
                updated_at_ms,
            ],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    replace_remote_message_children(connection, message_id, &message.key, message)?;
    refresh_message_fts(connection, message_id)?;
    Ok((inserted, false, message_id))
}

fn replace_remote_message_children(
    connection: &Connection,
    message_id: MessageId,
    key: &RemoteMessageKey,
    message: &RemoteMessage,
) -> Result<(), StorageError> {
    let id = message_id.to_string();
    connection
        .execute("DELETE FROM message_addresses WHERE message_id=?1", [&id])
        .map_err(|error| StorageError::from_sql(&error))?;
    for address in &message.mime.addresses {
        connection
            .execute(
                "INSERT INTO message_addresses(message_id, role, position, display_name, address)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    id,
                    mime_address_role_to_str(address.role),
                    address.position,
                    address.address.display_name,
                    address.address.address,
                ],
            )
            .map_err(|error| StorageError::from_sql(&error))?;
    }
    let mut old = connection
        .prepare(
            "SELECT provider_part_id, id, cache_key, checksum_sha256
             FROM attachments WHERE message_id=?1",
        )
        .map_err(|error| StorageError::from_sql(&error))?
        .query_map([&id], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(|error| StorageError::from_sql(&error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| StorageError::from_sql(&error))?;
    connection
        .execute("DELETE FROM attachments WHERE message_id=?1", [&id])
        .map_err(|error| StorageError::from_sql(&error))?;
    for attachment in &message.mime.attachments {
        let prior = old
            .iter_mut()
            .find(|(part, _, _, _)| part.as_deref() == Some(attachment.part_id.as_str()));
        let (attachment_id, cache_key, old_checksum) = prior.map_or_else(
            || (AttachmentId::new().to_string(), None, None),
            |(_, attachment_id, cache_key, checksum)| {
                let result = (attachment_id.clone(), cache_key.take(), checksum.take());
                *attachment_id = String::new();
                result
            },
        );
        let size_bytes = attachment
            .size_bytes
            .map(i64::try_from)
            .transpose()
            .map_err(|_| StorageError::Constraint)?;
        connection
            .execute(
                "INSERT INTO attachments(
                    id, message_id, provider_part_id, filename, media_type, size_bytes,
                    content_id, is_inline, cache_key, checksum_sha256
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    attachment_id,
                    id,
                    attachment.part_id,
                    attachment.file_name,
                    attachment.media_type,
                    size_bytes,
                    attachment.content_id,
                    attachment.inline,
                    cache_key,
                    attachment
                        .checksum_sha256
                        .as_ref()
                        .or(old_checksum.as_ref()),
                ],
            )
            .map_err(|error| StorageError::from_sql(&error))?;
    }
    for (_, attachment_id, cache_key, _) in old {
        if !attachment_id.is_empty()
            && let Some(cache_key) = cache_key
        {
            connection
                .execute(
                    "INSERT OR IGNORE INTO attachment_cleanup_queue(account_id, cache_key, created_at_ms)
                     VALUES (?1, ?2, (SELECT updated_at_ms FROM messages WHERE id=?3))",
                    params![key.account_id.to_string(), cache_key, id],
                )
                .map_err(|error| StorageError::from_sql(&error))?;
        }
    }
    Ok(())
}

fn refresh_message_fts(connection: &Connection, message_id: MessageId) -> Result<(), StorageError> {
    connection
        .execute(
            "DELETE FROM email_fts WHERE message_row_id=(SELECT row_id FROM messages WHERE id=?1)",
            [message_id.to_string()],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    let row = connection
        .query_row(
            "SELECT m.row_id, m.subject, m.body_plain, m.body_html,
                    coalesce((SELECT coalesce(a.display_name, '') || ' ' || a.address
                              FROM message_addresses a
                              WHERE a.message_id=m.id AND a.role IN ('from', 'sender')
                              ORDER BY CASE a.role WHEN 'from' THEN 0 ELSE 1 END,
                                       a.position LIMIT 1), '')
             FROM messages m WHERE m.id=?1",
            [message_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    let body = format!(
        "{} {}",
        row.2.as_deref().unwrap_or_default(),
        strip_html(row.3.as_deref().unwrap_or_default())
    );
    connection
        .execute(
            "INSERT INTO email_fts(message_row_id, subject, body, sender)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                row.0,
                search_document(&row.1),
                search_document(&body),
                search_document(&row.4),
            ],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    Ok(())
}

fn apply_remote_read_state(
    connection: &Connection,
    key: &RemoteMessageKey,
    read: bool,
    revision: Option<&ProviderRevision>,
    updated_at_ms: i64,
) -> Result<bool, StorageError> {
    let mapping: Option<String> = connection
        .query_row(
            "SELECT message_id FROM remote_message_ids
             WHERE account_id=?1 AND provider_mailbox_id=?2 AND provider_message_id=?3",
            params![
                key.account_id.to_string(),
                key.provider_mailbox_id,
                key.provider_message_id
            ],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))?;
    let Some(message_id) = mapping else {
        return Ok(false);
    };
    let pending: Option<bool> = connection
        .query_row(
            "SELECT desired_read FROM pending_read_mutations WHERE message_id=?1",
            [&message_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))?;
    connection
        .execute(
            "UPDATE messages SET remote_is_read=?2,
                    is_read=CASE WHEN ?3 IS NULL THEN ?2 ELSE is_read END,
                    provider_revision=?4,
                    updated_at_ms=max(updated_at_ms, created_at_ms, ?5) WHERE id=?1",
            params![
                message_id,
                read,
                pending,
                revision.map(ProviderRevision::expose),
                updated_at_ms
            ],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    Ok(false)
}

fn apply_remote_gone(
    connection: &Connection,
    key: &RemoteMessageKey,
) -> Result<bool, StorageError> {
    connection
        .execute(
            "DELETE FROM messages WHERE id=(SELECT message_id FROM remote_message_ids
             WHERE account_id=?1 AND provider_mailbox_id=?2 AND provider_message_id=?3)",
            params![
                key.account_id.to_string(),
                key.provider_mailbox_id,
                key.provider_message_id
            ],
        )
        .map(|changed| changed > 0)
        .map_err(|error| StorageError::from_sql(&error))
}

fn load_cleanup(
    connection: &Connection,
    account_id: AccountId,
) -> Result<Option<(String, Vec<String>, String)>, StorageError> {
    let row: Option<(String, String, String)> = connection
        .query_row(
            "SELECT credential_ref, attachment_cache_keys_json, stage
             FROM account_cleanup WHERE account_id=?1",
            [account_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))?;
    row.map(|(credential_ref, encoded, stage)| {
        Ok((
            credential_ref,
            serde_json::from_str(&encoded).map_err(|_| StorageError::Serialization)?,
            stage,
        ))
    })
    .transpose()
}

fn load_account_cache_keys(
    connection: &Connection,
    account_id: AccountId,
) -> Result<Vec<String>, StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT DISTINCT a.cache_key FROM attachments a
             JOIN messages m ON m.id=a.message_id
             WHERE m.account_id=?1 AND a.cache_key IS NOT NULL
             UNION
             SELECT cache_key FROM attachment_cleanup_queue WHERE account_id=?1
             ORDER BY 1",
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    statement
        .query_map([account_id.to_string()], |row| row.get(0))
        .map_err(|error| StorageError::from_sql(&error))?
        .map(|row| row.map_err(|error| StorageError::from_sql(&error)))
        .collect()
}

fn queue_replaced_attachment_keys(
    connection: &Connection,
    account_id: AccountId,
    message_id: &str,
    replacements: &[unimail_core::AttachmentInput],
) -> Result<(), StorageError> {
    let retained = replacements
        .iter()
        .filter_map(|attachment| attachment.cache_key.as_deref())
        .collect::<std::collections::HashSet<_>>();
    let mut statement = connection
        .prepare("SELECT cache_key FROM attachments WHERE message_id=?1 AND cache_key IS NOT NULL")
        .map_err(|error| StorageError::from_sql(&error))?;
    let rows = statement
        .query_map([message_id], |row| row.get::<_, String>(0))
        .map_err(|error| StorageError::from_sql(&error))?;
    let old_keys = rows
        .map(|row| row.map_err(|error| StorageError::from_sql(&error)))
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    for cache_key in old_keys {
        if !retained.contains(cache_key.as_str()) {
            connection
                .execute(
                    "INSERT OR IGNORE INTO attachment_cleanup_queue(account_id, cache_key, created_at_ms)
                     VALUES (?1, ?2, (SELECT updated_at_ms FROM messages WHERE id=?3))",
                    params![account_id.to_string(), cache_key, message_id],
                )
                .map_err(|error| StorageError::from_sql(&error))?;
        }
    }
    Ok(())
}

fn reject_account_cleanup_key(
    connection: &Connection,
    cache_key: &str,
) -> Result<(), StorageError> {
    let planned: bool = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM account_cleanup c, json_each(c.attachment_cache_keys_json) item
                WHERE item.value=?1
             )",
            [cache_key],
            |row| row.get(0),
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    if planned {
        Err(StorageError::Constraint)
    } else {
        Ok(())
    }
}

fn validate_cache_key(cache_key: &str) -> Result<(), StorageError> {
    let path = Path::new(cache_key);
    if cache_key.is_empty()
        || path.is_absolute()
        || !matches!(
            path.components().collect::<Vec<_>>().as_slice(),
            [Component::Normal(_)]
        )
    {
        Err(StorageError::Constraint)
    } else {
        Ok(())
    }
}

fn is_owned_transfer_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.starts_with(ATTACHMENT_PARTIAL_PREFIX))
}

fn remove_owned_transfer_file(path: &Path) -> RepositoryResult<()> {
    if !is_owned_transfer_path(path) {
        return Err(RepositoryError::ConstraintViolation);
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => Ok(()),
        Ok(_) => fs::remove_file(path).map_err(|_| RepositoryError::CleanupPending),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(RepositoryError::CleanupPending),
    }
}

fn parse_id<T>(value: &str) -> Result<T, StorageError>
where
    T: FromStr,
{
    T::from_str(value).map_err(|_| StorageError::Serialization)
}

fn parse_id_sql<T>(value: &str) -> rusqlite::Result<T>
where
    T: FromStr,
{
    T::from_str(value).map_err(|_| rusqlite::Error::InvalidQuery)
}

fn storage_to_sql(_error: StorageError) -> rusqlite::Error {
    rusqlite::Error::InvalidQuery
}

fn nonempty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

const fn provider_to_str(value: Provider) -> &'static str {
    match value {
        Provider::Gmail => "gmail",
        Provider::Outlook => "outlook",
        Provider::Qq => "qq",
        Provider::Netease => "netease",
    }
}

fn provider_from_str(value: &str) -> Result<Provider, StorageError> {
    match value {
        "gmail" => Ok(Provider::Gmail),
        "outlook" => Ok(Provider::Outlook),
        "qq" => Ok(Provider::Qq),
        "netease" => Ok(Provider::Netease),
        _ => Err(StorageError::Serialization),
    }
}

const fn auth_state_to_str(value: AccountAuthState) -> &'static str {
    match value {
        AccountAuthState::Connected => "connected",
        AccountAuthState::NeedsAuthentication => "needs_auth",
        AccountAuthState::Unavailable => "unavailable",
    }
}

fn auth_state_from_str(value: &str) -> Result<AccountAuthState, StorageError> {
    match value {
        "connected" => Ok(AccountAuthState::Connected),
        "needs_auth" => Ok(AccountAuthState::NeedsAuthentication),
        "unavailable" => Ok(AccountAuthState::Unavailable),
        _ => Err(StorageError::Serialization),
    }
}

const fn mailbox_role_to_str(value: MailboxRole) -> &'static str {
    match value {
        MailboxRole::Inbox => "inbox",
        MailboxRole::Sent => "sent",
        MailboxRole::Other => "other",
    }
}

const fn direction_to_str(value: MessageDirection) -> &'static str {
    match value {
        MessageDirection::Incoming => "incoming",
        MessageDirection::Outgoing => "outgoing",
    }
}

fn direction_from_str(value: &str) -> Result<MessageDirection, StorageError> {
    match value {
        "incoming" => Ok(MessageDirection::Incoming),
        "outgoing" => Ok(MessageDirection::Outgoing),
        _ => Err(StorageError::Serialization),
    }
}

const fn address_role_to_str(value: AddressRole) -> &'static str {
    match value {
        AddressRole::From => "from",
        AddressRole::Sender => "sender",
        AddressRole::To => "to",
        AddressRole::Cc => "cc",
        AddressRole::Bcc => "bcc",
        AddressRole::ReplyTo => "reply_to",
    }
}

fn address_role_from_str(value: &str) -> Result<AddressRole, StorageError> {
    match value {
        "from" => Ok(AddressRole::From),
        "sender" => Ok(AddressRole::Sender),
        "to" => Ok(AddressRole::To),
        "cc" => Ok(AddressRole::Cc),
        "bcc" => Ok(AddressRole::Bcc),
        "reply_to" => Ok(AddressRole::ReplyTo),
        _ => Err(StorageError::Serialization),
    }
}

const fn mime_address_role_to_str(value: MimeAddressRole) -> &'static str {
    match value {
        MimeAddressRole::From => "from",
        MimeAddressRole::Sender => "sender",
        MimeAddressRole::To => "to",
        MimeAddressRole::Cc => "cc",
        MimeAddressRole::Bcc => "bcc",
        MimeAddressRole::ReplyTo => "reply_to",
    }
}

const fn desired_read_state_to_str(value: DesiredReadMutationState) -> &'static str {
    match value {
        DesiredReadMutationState::Pending => "pending",
        DesiredReadMutationState::Running => "running",
        DesiredReadMutationState::WaitingBackoff => "waiting_backoff",
        DesiredReadMutationState::NeedsAuth => "needs_auth",
        DesiredReadMutationState::Failed => "failed",
    }
}

fn desired_read_state_from_str(value: &str) -> Result<DesiredReadMutationState, StorageError> {
    match value {
        "pending" => Ok(DesiredReadMutationState::Pending),
        "running" => Ok(DesiredReadMutationState::Running),
        "waiting_backoff" => Ok(DesiredReadMutationState::WaitingBackoff),
        "needs_auth" => Ok(DesiredReadMutationState::NeedsAuth),
        "failed" => Ok(DesiredReadMutationState::Failed),
        _ => Err(StorageError::Serialization),
    }
}

fn sync_mode_to_db(mode: SyncMode) -> (&'static str, Option<i64>) {
    match mode {
        SyncMode::Initial(limit) => ("initial", Some(i64::from(limit.get()))),
        SyncMode::Incremental => ("incremental", None),
        SyncMode::CursorReset(limit) => ("cursor_reset", Some(i64::from(limit.get()))),
    }
}

fn sync_mode_from_db(value: &str, limit: Option<i64>) -> Result<SyncMode, StorageError> {
    let bounded = || {
        let raw = limit.ok_or(StorageError::Serialization)?;
        let raw = u16::try_from(raw).map_err(|_| StorageError::Serialization)?;
        InitialSyncLimit::new(raw).map_err(|_| StorageError::Serialization)
    };
    match value {
        "initial" => bounded().map(SyncMode::Initial),
        "incremental" if limit.is_none() => Ok(SyncMode::Incremental),
        "cursor_reset" => bounded().map(SyncMode::CursorReset),
        _ => Err(StorageError::Serialization),
    }
}

const fn sync_state_to_db(lifecycle: SyncState) -> (&'static str, Option<&'static str>) {
    match lifecycle {
        SyncState::Scheduled => ("scheduled", None),
        SyncState::Running(current_stage) => ("running", Some(sync_stage_to_str(current_stage))),
        SyncState::WaitingBackoff => ("waiting_backoff", None),
        SyncState::Offline => ("offline", None),
        SyncState::NeedsAuth => ("needs_auth", None),
        SyncState::Committed => ("committed", None),
        SyncState::Failed => ("failed", None),
        SyncState::Cancelled => ("cancelled", None),
    }
}

const fn sync_stage_to_str(stage: SyncStage) -> &'static str {
    match stage {
        SyncStage::Load => "load",
        SyncStage::Fetch => "fetch",
        SyncStage::Commit => "commit",
        SyncStage::FlushReadMutations => "flush_read_mutations",
    }
}

fn sync_state_from_db(value: &str, stage: Option<&str>) -> Result<SyncState, StorageError> {
    match value {
        "scheduled" => Ok(SyncState::Scheduled),
        "running" => match stage {
            Some("load") => Ok(SyncState::Running(SyncStage::Load)),
            Some("fetch") => Ok(SyncState::Running(SyncStage::Fetch)),
            Some("commit") => Ok(SyncState::Running(SyncStage::Commit)),
            Some("flush_read_mutations") => Ok(SyncState::Running(SyncStage::FlushReadMutations)),
            _ => Err(StorageError::Serialization),
        },
        "waiting_backoff" => Ok(SyncState::WaitingBackoff),
        "offline" => Ok(SyncState::Offline),
        "needs_auth" => Ok(SyncState::NeedsAuth),
        "committed" => Ok(SyncState::Committed),
        "failed" => Ok(SyncState::Failed),
        "cancelled" => Ok(SyncState::Cancelled),
        _ => Err(StorageError::Serialization),
    }
}

fn map_storage_error(error: StorageError) -> RepositoryError {
    match error {
        StorageError::CredentialStoreUnavailable | StorageError::UnsupportedPlatform => {
            RepositoryError::CredentialStoreUnavailable
        }
        StorageError::DatabaseKeyUnavailable => RepositoryError::DatabaseKeyUnavailable,
        StorageError::InvalidDatabaseKey => RepositoryError::DatabaseKeyInvalid,
        StorageError::DatabaseOpen => RepositoryError::DatabaseOpenFailed,
        StorageError::Migration => RepositoryError::MigrationFailed,
        StorageError::CipherUnavailable => RepositoryError::CipherUnavailable,
        StorageError::Fts5Unavailable => RepositoryError::Fts5Unavailable,
        StorageError::LockPoisoned => RepositoryError::StorageBusy,
        StorageError::Constraint => RepositoryError::ConstraintViolation,
        StorageError::DraftRevisionConflict => RepositoryError::RevisionConflict,
        StorageError::NotFound => RepositoryError::NotFound,
        StorageError::Serialization => RepositoryError::InvalidData,
    }
}

#[cfg(test)]
mod tests {
    use std::{io::Write, str::FromStr, sync::Arc};

    #[cfg(unix)]
    use std::os::unix::fs::{PermissionsExt, symlink};

    use secrecy::SecretBox;
    use tempfile::TempDir;
    use unimail_core::{
        AccountAuthState, AccountAuthUpdateInput, AccountConnectInput, AccountCreateInput,
        AccountId, AddressRole, AttachmentId, AttachmentInput, AttachmentVerificationInput,
        ClaimDesiredReadMutationInput, ClaimSyncOperationInput, CompleteDesiredReadMutationInput,
        CredentialRef, CredentialStore, DesiredReadMutationState, DraftAddress, DraftId,
        DraftSaveInput, DraftSendReviewKey, DurableCheckpoint, InboxListInput, InitialSyncLimit,
        LeaseId, MailboxId, MailboxRole, MailboxUpsertInput, MessageAddressInput, MessageDirection,
        MessageId, MessageListInput, MessageReadStateInput, MessageUpsertInput, MimeAddress,
        MimeAddressEntry, MimeAddressRole, MimeBody, NormalizedMimeMessage,
        OfflineDraftReviewInput, OpaqueProviderCursor, OperationId, OperationLease, Provider,
        ProviderRevision, RemoteChange, RemoteMailbox, RemoteMailboxKey, RemoteMessage,
        RemoteMessageKey, RepositoryError, SafeErrorCode, ScheduleSyncInput, SearchMessagesInput,
        StorageRepository, SyncBatchInput, SyncBatchResult, SyncCursorKey, SyncMode, SyncState,
        SyncTrigger, TransitionDesiredReadMutationInput, TransitionSyncOperationInput,
    };

    use super::{ATTACHMENT_PARTIAL_PREFIX, SqlCipherRepository};
    use crate::FakeCredentialStore;

    struct Fixture {
        directory: TempDir,
        path: std::path::PathBuf,
        credentials: FakeCredentialStore,
        repository: SqlCipherRepository,
        account_id: AccountId,
        mailbox_id: MailboxId,
    }

    fn fixture() -> Fixture {
        let directory = tempfile::tempdir().expect("temporary profile");
        let path = directory.path().join("unimail.db");
        let credentials = FakeCredentialStore::new();
        let repository = SqlCipherRepository::initialize(
            &path,
            Arc::new(credentials.clone()) as Arc<dyn CredentialStore>,
        )
        .expect("initialize repository");
        let account_id = AccountId::new();
        let credential_ref = CredentialRef::new(format!("account-{account_id}"));
        credentials
            .put(
                &credential_ref,
                SecretBox::new(vec![3_u8; 16].into_boxed_slice()),
            )
            .expect("seed account credential");
        repository
            .create_account(AccountCreateInput {
                id: account_id,
                provider: Provider::Gmail,
                email: format!("{account_id}@example.test"),
                display_name: Some("测试账户".to_owned()),
                credential_ref,
                auth_state: AccountAuthState::Connected,
                enabled: true,
                created_at_ms: 1,
            })
            .expect("create account");
        let mailbox_id = MailboxId::new();
        repository
            .upsert_mailbox(MailboxUpsertInput {
                id: mailbox_id,
                account_id,
                provider_mailbox_id: "inbox".to_owned(),
                role: MailboxRole::Inbox,
                display_name: "收件箱".to_owned(),
                updated_at_ms: 1,
            })
            .expect("create mailbox");
        Fixture {
            directory,
            path,
            credentials,
            repository,
            account_id,
            mailbox_id,
        }
    }

    #[cfg(unix)]
    fn permission_mode(path: &std::path::Path) -> u32 {
        std::fs::symlink_metadata(path)
            .expect("sensitive path metadata")
            .permissions()
            .mode()
            & 0o777
    }

    #[cfg(unix)]
    #[test]
    fn unix_attachment_storage_permissions_are_enforced_end_to_end() {
        let fixture = fixture();
        let attachment_cache = fixture.directory.path().join("attachments");
        assert_eq!(permission_mode(&attachment_cache), 0o700);

        let destination = fixture.directory.path().join("private-attachment.bin");
        let mut transfer = fixture
            .repository
            .begin_attachment_transfer(OperationId::new(), &destination, 1)
            .expect("begin private attachment transfer");
        assert_eq!(permission_mode(&transfer.temporary_path), 0o600);
        let mut file = transfer.take_file().expect("take private transfer file");
        file.write_all(b"private attachment")
            .expect("write private attachment");
        file.sync_all().expect("sync private attachment");
        drop(file);
        fixture
            .repository
            .finish_attachment_transfer(&transfer)
            .expect("finish private attachment transfer");
        assert_eq!(permission_mode(&destination), 0o600);

        let symlink_directory = tempfile::tempdir().expect("temporary symlink profile");
        let profile_directory = symlink_directory.path().join("profile");
        let unrelated_cache = symlink_directory.path().join("unrelated-cache");
        std::fs::create_dir(&profile_directory).expect("profile directory");
        std::fs::create_dir(&unrelated_cache).expect("unrelated cache directory");
        std::fs::set_permissions(&unrelated_cache, std::fs::Permissions::from_mode(0o755))
            .expect("set unrelated cache permissions");
        symlink(&unrelated_cache, profile_directory.join("attachments"))
            .expect("attachment cache symlink");

        assert!(matches!(
            SqlCipherRepository::initialize(
                profile_directory.join("unimail.db"),
                Arc::new(FakeCredentialStore::new()) as Arc<dyn CredentialStore>,
            ),
            Err(RepositoryError::DatabaseOpenFailed)
        ));
        assert_eq!(permission_mode(&unrelated_cache), 0o755);
    }

    #[test]
    fn account_connection_creates_or_reconnects_atomically_without_secret_columns() {
        let fixture = fixture();
        let email = format!("{}@example.test", fixture.account_id);
        let replacement = CredentialRef::new("gmail-reconnected-credential");
        let result = fixture
            .repository
            .connect_account(AccountConnectInput {
                id: AccountId::new(),
                provider: Provider::Gmail,
                email: email.clone(),
                display_name: Some("重新连接".to_owned()),
                credential_ref: replacement.clone(),
                connected_at_ms: 20,
            })
            .expect("reconnect existing account");

        assert!(!result.created);
        assert_eq!(result.account.id, fixture.account_id);
        assert_eq!(result.account.credential_ref, replacement);
        assert_eq!(result.account.display_name.as_deref(), Some("重新连接"));
        assert!(result.replaced_credential_ref.is_some());

        let error_code = SafeErrorCode::new("gmail_authentication_required")
            .expect("allowlisted Gmail authentication code");
        let updated = fixture
            .repository
            .update_account_auth(AccountAuthUpdateInput {
                account_id: fixture.account_id,
                auth_state: AccountAuthState::NeedsAuthentication,
                safe_error_code: Some(error_code),
                updated_at_ms: 21,
            })
            .expect("update authentication state");
        assert_eq!(updated.auth_state, AccountAuthState::NeedsAuthentication);
        assert_eq!(
            updated.last_error_code.as_deref(),
            Some("gmail_authentication_required")
        );

        assert_eq!(
            fixture
                .repository
                .list_accounts()
                .expect("list connected account")
                .iter()
                .filter(|account| account.provider == Provider::Gmail && account.email == email)
                .count(),
            1
        );
    }

    #[test]
    fn runnable_and_claim_paths_enforce_the_account_provider() {
        let fixture = fixture();
        let outlook_id = AccountId::new();
        fixture
            .repository
            .create_account(AccountCreateInput {
                id: outlook_id,
                provider: Provider::Outlook,
                email: "outlook@example.test".to_owned(),
                display_name: None,
                credential_ref: CredentialRef::new("outlook-credential"),
                auth_state: AccountAuthState::Connected,
                enabled: true,
                created_at_ms: 2,
            })
            .expect("create Outlook account");
        let gmail_operation = OperationId::new();
        let outlook_operation = OperationId::new();
        for (operation_id, account_id) in [
            (gmail_operation, fixture.account_id),
            (outlook_operation, outlook_id),
        ] {
            fixture
                .repository
                .schedule_sync_operation(ScheduleSyncInput {
                    operation_id,
                    account_id,
                    scope: "inbox".to_owned(),
                    trigger: SyncTrigger::Manual,
                    mode: SyncMode::Initial(InitialSyncLimit::new(500).expect("limit")),
                    scheduled_at_ms: 10,
                })
                .expect("schedule provider operation");
        }

        let gmail = fixture
            .repository
            .list_runnable_sync_operations(Provider::Gmail, 10, 10)
            .expect("list Gmail operations");
        let outlook = fixture
            .repository
            .list_runnable_sync_operations(Provider::Outlook, 10, 10)
            .expect("list Outlook operations");
        assert_eq!(gmail.len(), 1);
        assert_eq!(gmail[0].operation_id, gmail_operation);
        assert_eq!(outlook.len(), 1);
        assert_eq!(outlook[0].operation_id, outlook_operation);

        let wrong_provider = fixture
            .repository
            .claim_sync_operation(ClaimSyncOperationInput {
                operation_id: gmail_operation,
                provider: Provider::Outlook,
                lease: OperationLease {
                    id: LeaseId::new(),
                    expires_at_ms: 100,
                },
                claimed_at_ms: 10,
            })
            .expect("mismatched claim is safe");
        assert!(wrong_provider.is_none());
        assert!(
            fixture
                .repository
                .claim_sync_operation(ClaimSyncOperationInput {
                    operation_id: gmail_operation,
                    provider: Provider::Gmail,
                    lease: OperationLease {
                        id: LeaseId::new(),
                        expires_at_ms: 100,
                    },
                    claimed_at_ms: 10,
                })
                .expect("matching claim")
                .is_some()
        );
    }

    fn message(
        id: MessageId,
        account_id: AccountId,
        mailbox_id: MailboxId,
        provider_id: &str,
        subject: &str,
        received_at_ms: i64,
    ) -> MessageUpsertInput {
        MessageUpsertInput {
            id,
            account_id,
            mailbox_id,
            provider_message_id: provider_id.to_owned(),
            provider_revision: Some("1".to_owned()),
            thread_id: None,
            rfc_message_id: None,
            subject: Some(subject.to_owned()),
            snippet: Some(subject.to_owned()),
            plain_body: Some(format!("正文 {subject}")),
            html_body: None,
            read: false,
            direction: MessageDirection::Incoming,
            sent_at_ms: None,
            received_at_ms,
            parser_version: 1,
            sanitizer_version: 1,
            addresses: vec![MessageAddressInput {
                role: AddressRole::From,
                position: 0,
                display_name: Some("发件人".to_owned()),
                address: "sender@example.test".to_owned(),
            }],
            attachments: Vec::new(),
            updated_at_ms: received_at_ms,
        }
    }

    fn remote_message(
        account_id: AccountId,
        provider_mailbox_id: &str,
        provider_message_id: &str,
        read: bool,
    ) -> RemoteMessage {
        RemoteMessage {
            key: RemoteMessageKey {
                account_id,
                provider_mailbox_id: provider_mailbox_id.to_owned(),
                provider_message_id: provider_message_id.to_owned(),
            },
            provider_revision: None,
            provider_thread_id: Some("thread-1".to_owned()),
            read,
            sent_at_ms: None,
            received_at_ms: 10,
            mime: NormalizedMimeMessage {
                subject: Some("Remote subject".to_owned()),
                message_id: Some("remote@example.test".to_owned()),
                in_reply_to: None,
                references: vec!["root@example.test".to_owned()],
                addresses: vec![MimeAddressEntry {
                    role: MimeAddressRole::Sender,
                    position: 0,
                    address: MimeAddress {
                        display_name: Some("Sender".to_owned()),
                        address: "sender@example.test".to_owned(),
                    },
                }],
                body: MimeBody {
                    plain: Some("Remote body".to_owned()),
                    html: None,
                },
                attachments: Vec::new(),
            },
        }
    }

    fn commit_remote_changes(
        fixture: &Fixture,
        provider_mailbox_id: &str,
        changes: Vec<RemoteChange>,
        now_ms: i64,
    ) -> SyncBatchResult {
        let operation_id = OperationId::new();
        fixture
            .repository
            .schedule_sync_operation(ScheduleSyncInput {
                operation_id,
                account_id: fixture.account_id,
                scope: "inbox".to_owned(),
                trigger: SyncTrigger::Manual,
                mode: SyncMode::Incremental,
                scheduled_at_ms: now_ms,
            })
            .expect("schedule remote changes");
        let lease = OperationLease {
            id: LeaseId::new(),
            expires_at_ms: now_ms + 100,
        };
        fixture
            .repository
            .claim_sync_operation(ClaimSyncOperationInput {
                operation_id,
                provider: Provider::Gmail,
                lease,
                claimed_at_ms: now_ms,
            })
            .expect("claim remote changes")
            .expect("claimed remote changes");
        fixture
            .repository
            .commit_sync_batch(SyncBatchInput {
                operation_id,
                lease_id: lease.id,
                cursor_key: SyncCursorKey {
                    account_id: fixture.account_id,
                    scope: "inbox".to_owned(),
                },
                mailboxes: vec![RemoteMailbox {
                    key: RemoteMailboxKey {
                        account_id: fixture.account_id,
                        provider_mailbox_id: provider_mailbox_id.to_owned(),
                    },
                    role: MailboxRole::Inbox,
                    display_name: provider_mailbox_id.to_owned(),
                }],
                changes,
                checkpoint: DurableCheckpoint::new(
                    OpaqueProviderCursor::from_json(format!("{{\"at\":{now_ms}}}"))
                        .expect("checkpoint"),
                ),
                committed_at_ms: now_ms,
            })
            .expect("commit remote changes")
    }

    fn create_inbox_account(
        fixture: &Fixture,
        provider: Provider,
        email: &str,
        credential_ref: &str,
        enabled: bool,
        created_at_ms: i64,
    ) -> (AccountId, MailboxId) {
        let account_id = AccountId::new();
        fixture
            .repository
            .create_account(AccountCreateInput {
                id: account_id,
                provider,
                email: email.to_owned(),
                display_name: None,
                credential_ref: CredentialRef::new(credential_ref),
                auth_state: AccountAuthState::Connected,
                enabled,
                created_at_ms,
            })
            .expect("create Inbox test account");
        let mailbox_id = MailboxId::new();
        fixture
            .repository
            .upsert_mailbox(MailboxUpsertInput {
                id: mailbox_id,
                account_id,
                provider_mailbox_id: format!("{provider:?}-inbox").to_ascii_lowercase(),
                role: MailboxRole::Inbox,
                display_name: "收件箱".to_owned(),
                updated_at_ms: created_at_ms,
            })
            .expect("create Inbox test mailbox");
        (account_id, mailbox_id)
    }

    struct UnifiedInboxScenario {
        fixture: Fixture,
        first_same_time: MessageId,
        second_same_time: MessageId,
        outlook_message: MessageId,
        outlook_id: AccountId,
    }

    fn seed_unified_inbox_scenario() -> UnifiedInboxScenario {
        let fixture = fixture();
        let first_same_time =
            MessageId::from_str("00000000-0000-4000-8000-000000000003").expect("message id");
        let second_same_time =
            MessageId::from_str("00000000-0000-4000-8000-000000000002").expect("message id");
        fixture
            .repository
            .upsert_message(message(
                first_same_time,
                fixture.account_id,
                fixture.mailbox_id,
                "gmail-unread",
                "Gmail 未读",
                10,
            ))
            .expect("seed first Gmail message");
        let mut read_message = message(
            second_same_time,
            fixture.account_id,
            fixture.mailbox_id,
            "gmail-read",
            "Gmail 已读",
            10,
        );
        read_message.read = true;
        fixture
            .repository
            .upsert_message(read_message)
            .expect("seed read Gmail message");

        let (outlook_id, outlook_inbox) = create_inbox_account(
            &fixture,
            Provider::Outlook,
            "outlook-inbox@example.test",
            "outlook-inbox-credential",
            true,
            2,
        );
        let outlook_message = MessageId::new();
        fixture
            .repository
            .upsert_message(message(
                outlook_message,
                outlook_id,
                outlook_inbox,
                "outlook-unread",
                "Outlook 未读",
                11,
            ))
            .expect("seed Outlook message");
        let outlook_sent = MailboxId::new();
        fixture
            .repository
            .upsert_mailbox(MailboxUpsertInput {
                id: outlook_sent,
                account_id: outlook_id,
                provider_mailbox_id: "outlook-sent".to_owned(),
                role: MailboxRole::Sent,
                display_name: "已发送".to_owned(),
                updated_at_ms: 2,
            })
            .expect("create Outlook Sent");
        fixture
            .repository
            .upsert_message(message(
                MessageId::new(),
                outlook_id,
                outlook_sent,
                "outlook-sent-message",
                "不应进入收件箱",
                20,
            ))
            .expect("seed Sent message");

        let (disabled_id, disabled_inbox) = create_inbox_account(
            &fixture,
            Provider::Qq,
            "disabled@example.test",
            "disabled-credential",
            false,
            3,
        );
        fixture
            .repository
            .upsert_message(message(
                MessageId::new(),
                disabled_id,
                disabled_inbox,
                "disabled-message",
                "禁用账户邮件",
                30,
            ))
            .expect("seed disabled message");

        UnifiedInboxScenario {
            fixture,
            first_same_time,
            second_same_time,
            outlook_message,
            outlook_id,
        }
    }

    fn inbox_ids(page: &unimail_core::MessagePage) -> Vec<MessageId> {
        page.items.iter().map(|item| item.id).collect()
    }

    #[test]
    fn unified_inbox_filters_accounts_read_state_and_pages_stably() {
        let scenario = seed_unified_inbox_scenario();
        let first = scenario
            .fixture
            .repository
            .list_inbox_messages(&InboxListInput {
                account_id: None,
                unread_only: false,
                before: None,
                limit: 2,
            })
            .expect("first unified page");
        assert_eq!(
            inbox_ids(&first),
            [scenario.outlook_message, scenario.first_same_time]
        );
        let second = scenario
            .fixture
            .repository
            .list_inbox_messages(&InboxListInput {
                account_id: None,
                unread_only: false,
                before: first.next,
                limit: 2,
            })
            .expect("second unified page");
        assert_eq!(inbox_ids(&second), [scenario.second_same_time]);
        assert!(second.next.is_none());

        let unread = scenario
            .fixture
            .repository
            .list_inbox_messages(&InboxListInput {
                account_id: None,
                unread_only: true,
                before: None,
                limit: 100,
            })
            .expect("unread unified page");
        assert_eq!(
            inbox_ids(&unread),
            [scenario.outlook_message, scenario.first_same_time]
        );

        let gmail = scenario
            .fixture
            .repository
            .list_inbox_messages(&InboxListInput {
                account_id: Some(scenario.fixture.account_id),
                unread_only: false,
                before: None,
                limit: 100,
            })
            .expect("Gmail Inbox page");
        assert_eq!(
            inbox_ids(&gmail),
            [scenario.first_same_time, scenario.second_same_time]
        );

        scenario
            .fixture
            .repository
            .store
            .with_connection(|connection| {
                connection
                    .execute(
                        "UPDATE accounts SET deleting=1 WHERE id=?1",
                        [scenario.outlook_id.to_string()],
                    )
                    .map_err(|error| crate::StorageError::from_sql(&error))?;
                Ok(())
            })
            .expect("mark Outlook account deleting");
        let after_delete_started = scenario
            .fixture
            .repository
            .list_inbox_messages(&InboxListInput {
                account_id: None,
                unread_only: false,
                before: None,
                limit: 100,
            })
            .expect("unified page hides deleting account");
        assert_eq!(
            inbox_ids(&after_delete_started),
            [scenario.first_same_time, scenario.second_same_time]
        );
    }

    #[test]
    fn local_search_is_safe_cjk_capable_scoped_and_paged() {
        let scenario = seed_unified_inbox_scenario();
        let first = scenario
            .fixture
            .repository
            .search_inbox_messages(&SearchMessagesInput {
                query: "未读".to_owned(),
                account_id: None,
                unread_only: false,
                after: None,
                limit: 1,
            })
            .expect("first search page");
        assert_eq!(first.items.len(), 1);
        assert_eq!(first.items[0].summary.id, scenario.outlook_message);
        assert!(first.items[0].match_context.is_some());
        let second = scenario
            .fixture
            .repository
            .search_inbox_messages(&SearchMessagesInput {
                query: "未读".to_owned(),
                account_id: None,
                unread_only: false,
                after: first.next,
                limit: 1,
            })
            .expect("second search page");
        assert_eq!(second.items.len(), 1);
        assert_eq!(second.items[0].summary.id, scenario.first_same_time);

        let gmail_only = scenario
            .fixture
            .repository
            .search_inbox_messages(&SearchMessagesInput {
                query: "sender example test".to_owned(),
                account_id: Some(scenario.fixture.account_id),
                unread_only: true,
                after: None,
                limit: 100,
            })
            .expect("account-scoped sender search");
        assert_eq!(gmail_only.items.len(), 1);
        assert_eq!(gmail_only.items[0].summary.id, scenario.first_same_time);

        let operator_text = scenario
            .fixture
            .repository
            .search_inbox_messages(&SearchMessagesInput {
                query: "\" OR * NEAR(".to_owned(),
                account_id: None,
                unread_only: false,
                after: None,
                limit: 100,
            })
            .expect("operators are literal terms");
        assert!(operator_text.items.is_empty());
    }

    #[test]
    fn attachment_source_and_verification_are_safe() {
        let fixture = fixture();
        let attachment_id = AttachmentId::new();
        let mut input = message(
            MessageId::new(),
            fixture.account_id,
            fixture.mailbox_id,
            "attachment-message",
            "附件邮件",
            10,
        );
        input.attachments.push(AttachmentInput {
            id: attachment_id,
            provider_part_id: Some("part-1".to_owned()),
            file_name: Some("report.txt".to_owned()),
            media_type: "text/plain".to_owned(),
            size_bytes: Some(5),
            content_id: None,
            inline: false,
            cache_key: None,
            checksum_sha256: None,
        });
        fixture
            .repository
            .upsert_message(input)
            .expect("seed attachment message");
        let source = fixture
            .repository
            .get_attachment_download_source(attachment_id)
            .expect("load attachment source")
            .expect("attachment source");
        assert_eq!(source.provider, Provider::Gmail);
        assert_eq!(source.provider_part_id, "part-1");
        assert_eq!(source.size_bytes, Some(5));

        fixture
            .repository
            .record_attachment_verification(AttachmentVerificationInput {
                attachment_id,
                size_bytes: 5,
                checksum_sha256: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
                    .to_owned(),
            })
            .expect("record verification");
    }

    #[test]
    fn attachment_transfer_and_restart_cleanup_are_safe() {
        let fixture = fixture();
        let destination = fixture.directory.path().join("report.txt");
        let mut transfer = fixture
            .repository
            .begin_attachment_transfer(OperationId::new(), &destination, 20)
            .expect("begin transfer");
        let mut file = transfer.take_file().expect("take transfer file");
        file.write_all(b"hello").expect("write transfer");
        file.sync_all().expect("sync transfer");
        drop(file);
        fixture
            .repository
            .finish_attachment_transfer(&transfer)
            .expect("finish transfer");
        assert_eq!(std::fs::read(&destination).expect("saved file"), b"hello");
        assert!(matches!(
            fixture
                .repository
                .begin_attachment_transfer(OperationId::new(), &destination, 21),
            Err(RepositoryError::ConstraintViolation)
        ));

        let interrupted_destination = fixture.directory.path().join("interrupted.txt");
        let mut interrupted = fixture
            .repository
            .begin_attachment_transfer(OperationId::new(), interrupted_destination, 22)
            .expect("begin interrupted transfer");
        interrupted
            .take_file()
            .expect("take interrupted file")
            .write_all(b"partial")
            .expect("write partial");
        drop(interrupted);
        let partial_before = std::fs::read_dir(fixture.directory.path())
            .expect("list partial directory")
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(ATTACHMENT_PARTIAL_PREFIX)
            })
            .count();
        assert_eq!(partial_before, 1);
        drop(fixture.repository);
        SqlCipherRepository::initialize(
            &fixture.path,
            Arc::new(fixture.credentials.clone()) as Arc<dyn CredentialStore>,
        )
        .expect("restart repository");
        let partial_after = std::fs::read_dir(fixture.directory.path())
            .expect("list recovered directory")
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(ATTACHMENT_PARTIAL_PREFIX)
            })
            .count();
        assert_eq!(partial_after, 0);
    }

    #[test]
    fn search_context_uses_the_field_that_contains_a_query_term() {
        assert_eq!(
            super::search_match_context(
                "needle",
                Some("unrelated subject"),
                Some("unrelated sender"),
                Some("sender@example.test"),
                Some("leading needle body"),
                None,
            ),
            Some("leading needle body".to_owned())
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn message_upsert_paging_fts_rebuild_and_cascade_are_consistent() {
        let fixture = fixture();
        let ids = [
            MessageId::from_str("00000000-0000-4000-8000-000000000003").expect("id"),
            MessageId::from_str("00000000-0000-4000-8000-000000000002").expect("id"),
            MessageId::from_str("00000000-0000-4000-8000-000000000001").expect("id"),
        ];
        for (index, id) in ids.into_iter().enumerate() {
            fixture
                .repository
                .upsert_message(message(
                    id,
                    fixture.account_id,
                    fixture.mailbox_id,
                    &format!("remote-{index}"),
                    &format!("主题 {index}"),
                    10,
                ))
                .expect("upsert message");
        }
        let replacement_id = MessageId::new();
        let updated = fixture
            .repository
            .upsert_message(message(
                replacement_id,
                fixture.account_id,
                fixture.mailbox_id,
                "remote-0",
                "更新后的主题",
                11,
            ))
            .expect("idempotent upsert");
        assert!(!updated.inserted);
        assert_eq!(updated.message_id, ids[0]);
        assert_eq!(
            fixture
                .repository
                .search_message_ids("更新后的主题", 10)
                .expect("search"),
            [ids[0]]
        );

        let first = fixture
            .repository
            .list_messages(&MessageListInput {
                account_id: fixture.account_id,
                mailbox_id: None,
                before: None,
                limit: 2,
            })
            .expect("first page");
        assert_eq!(first.items.len(), 2);
        let second = fixture
            .repository
            .list_messages(&MessageListInput {
                account_id: fixture.account_id,
                mailbox_id: None,
                before: first.next,
                limit: 2,
            })
            .expect("second page");
        assert_eq!(second.items.len(), 1);
        assert!(!first.items.iter().any(|item| item.id == second.items[0].id));

        fixture
            .repository
            .store
            .with_connection(|connection| {
                connection
                    .execute("DELETE FROM email_fts", [])
                    .map_err(|error| crate::StorageError::from_sql(&error))?;
                Ok(())
            })
            .expect("corrupt fts projection");
        assert!(
            fixture
                .repository
                .search_message_ids("更新后的主题", 10)
                .expect("search missing projection")
                .is_empty()
        );
        fixture
            .repository
            .rebuild_search_index()
            .expect("rebuild fts");
        assert_eq!(
            fixture
                .repository
                .search_message_ids("更新后的主题", 10)
                .expect("search rebuilt"),
            [ids[0]]
        );
        fixture
            .repository
            .store
            .with_connection(|connection| {
                connection
                    .execute(
                        "DELETE FROM accounts WHERE id=?1",
                        [fixture.account_id.to_string()],
                    )
                    .map_err(|error| crate::StorageError::from_sql(&error))?;
                let message_count: i64 = connection
                    .query_row("SELECT count(*) FROM messages", [], |row| row.get(0))
                    .map_err(|error| crate::StorageError::from_sql(&error))?;
                let fts_count: i64 = connection
                    .query_row("SELECT count(*) FROM email_fts", [], |row| row.get(0))
                    .map_err(|error| crate::StorageError::from_sql(&error))?;
                assert_eq!(message_count, 0);
                assert_eq!(fts_count, 0);
                Ok(())
            })
            .expect("raw cascade");
        assert!(
            fixture
                .repository
                .search_message_ids("更新后的主题", 10)
                .expect("search after cascade")
                .is_empty()
        );
    }

    #[test]
    fn draft_revision_conflicts_are_detected() {
        let fixture = fixture();
        let draft_id = DraftId::new();
        let input = DraftSaveInput {
            id: draft_id,
            account_id: fixture.account_id,
            to: vec![DraftAddress {
                display_name: None,
                address: "to@example.test".to_owned(),
            }],
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: "草稿".to_owned(),
            plain_body: "正文".to_owned(),
            html_body: None,
            in_reply_to_message_id: None,
            attachments: Vec::new(),
            expected_revision: None,
            updated_at_ms: 2,
        };
        let created = fixture
            .repository
            .save_draft(input.clone())
            .expect("create draft");
        assert_eq!(created.revision, 1);
        let mut update = input.clone();
        update.expected_revision = Some(1);
        update.updated_at_ms = 3;
        assert_eq!(
            fixture
                .repository
                .save_draft(update)
                .expect("update draft")
                .revision,
            2
        );
        let mut stale = input;
        stale.expected_revision = Some(1);
        stale.updated_at_ms = 4;
        assert_eq!(
            fixture.repository.save_draft(stale),
            Err(RepositoryError::RevisionConflict)
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn cursor_and_message_batch_commit_or_rollback_together() {
        let fixture = fixture();
        let key = SyncCursorKey {
            account_id: fixture.account_id,
            scope: "inbox".to_owned(),
        };
        let operation_id = OperationId::new();
        fixture
            .repository
            .schedule_sync_operation(ScheduleSyncInput {
                operation_id,
                account_id: fixture.account_id,
                scope: key.scope.clone(),
                trigger: SyncTrigger::Manual,
                mode: SyncMode::Initial(InitialSyncLimit::new(500).expect("limit")),
                scheduled_at_ms: 4,
            })
            .expect("schedule");
        let lease = OperationLease {
            id: LeaseId::new(),
            expires_at_ms: 100,
        };
        fixture
            .repository
            .claim_sync_operation(ClaimSyncOperationInput {
                operation_id,
                provider: Provider::Gmail,
                lease,
                claimed_at_ms: 4,
            })
            .expect("claim")
            .expect("claimed operation");
        let checkpoint = DurableCheckpoint::new(
            OpaqueProviderCursor::from_json("{\"next\":1}").expect("checkpoint"),
        );
        let remote_key = RemoteMessageKey {
            account_id: fixture.account_id,
            provider_mailbox_id: "valid-batch".to_owned(),
            provider_message_id: "atomic".to_owned(),
        };
        let remote = RemoteMessage {
            key: remote_key.clone(),
            provider_revision: None,
            provider_thread_id: None,
            read: false,
            sent_at_ms: None,
            received_at_ms: 5,
            mime: NormalizedMimeMessage {
                subject: Some("原子性".to_owned()),
                message_id: None,
                in_reply_to: None,
                references: Vec::new(),
                addresses: vec![MimeAddressEntry {
                    role: MimeAddressRole::From,
                    position: 0,
                    address: MimeAddress {
                        display_name: None,
                        address: "sender@example.test".to_owned(),
                    },
                }],
                body: MimeBody {
                    plain: Some("正文".to_owned()),
                    html: None,
                },
                attachments: Vec::new(),
            },
        };
        let batch = SyncBatchInput {
            operation_id,
            lease_id: lease.id,
            cursor_key: key.clone(),
            mailboxes: vec![RemoteMailbox {
                key: RemoteMailboxKey {
                    account_id: fixture.account_id,
                    provider_mailbox_id: "valid-batch".to_owned(),
                },
                role: MailboxRole::Inbox,
                display_name: "Inbox".to_owned(),
            }],
            changes: vec![
                RemoteChange::Upsert(Box::new(remote.clone())),
                RemoteChange::Gone(RemoteMessageKey {
                    account_id: AccountId::new(),
                    provider_mailbox_id: "other".to_owned(),
                    provider_message_id: "invalid".to_owned(),
                }),
            ],
            checkpoint: checkpoint.clone(),
            committed_at_ms: 5,
        };
        assert_eq!(
            fixture.repository.commit_sync_batch(batch),
            Err(RepositoryError::ConstraintViolation)
        );
        assert!(
            fixture
                .repository
                .get_sync_cursor(&key)
                .expect("cursor read")
                .is_none()
        );
        let rolled_back: (i64, i64) = fixture
            .repository
            .store
            .with_connection(|connection| {
                Ok((
                    connection
                        .query_row(
                            "SELECT count(*) FROM messages WHERE provider_message_id='atomic'",
                            [],
                            |row| row.get(0),
                        )
                        .map_err(|error| crate::StorageError::from_sql(&error))?,
                    connection
                        .query_row(
                            "SELECT count(*) FROM email_fts WHERE email_fts MATCH '原子性'",
                            [],
                            |row| row.get(0),
                        )
                        .map_err(|error| crate::StorageError::from_sql(&error))?,
                ))
            })
            .expect("rolled back projections");
        assert_eq!(rolled_back, (0, 0));
        assert_eq!(
            fixture
                .repository
                .get_sync_operation(operation_id)
                .expect("operation query")
                .expect("operation")
                .state,
            SyncState::Running(unimail_core::SyncStage::Load)
        );

        let valid = SyncBatchInput {
            operation_id,
            lease_id: lease.id,
            cursor_key: key.clone(),
            mailboxes: vec![RemoteMailbox {
                key: RemoteMailboxKey {
                    account_id: fixture.account_id,
                    provider_mailbox_id: "valid-batch".to_owned(),
                },
                role: MailboxRole::Inbox,
                display_name: "Inbox".to_owned(),
            }],
            changes: vec![RemoteChange::Upsert(Box::new(remote))],
            checkpoint,
            committed_at_ms: 5,
        };
        fixture
            .repository
            .commit_sync_batch(valid)
            .expect("valid batch");
        assert_eq!(
            fixture
                .repository
                .get_sync_cursor(&key)
                .expect("cursor read")
                .expect("cursor")
                .checkpoint
                .cursor()
                .as_json(),
            "{\"next\":1}"
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn triggers_cancellation_and_account_leases_are_durable() {
        let fixture = fixture();
        let operation_id = OperationId::new();
        fixture
            .repository
            .schedule_sync_operation(ScheduleSyncInput {
                operation_id,
                account_id: fixture.account_id,
                scope: "inbox".to_owned(),
                trigger: SyncTrigger::Manual,
                mode: SyncMode::Incremental,
                scheduled_at_ms: 10,
            })
            .expect("schedule first");
        let first_lease = OperationLease {
            id: LeaseId::new(),
            expires_at_ms: 100,
        };
        let claimed = fixture
            .repository
            .claim_sync_operation(ClaimSyncOperationInput {
                operation_id,
                provider: Provider::Gmail,
                lease: first_lease,
                claimed_at_ms: 10,
            })
            .expect("claim first")
            .expect("claimed first");
        assert!(claimed.triggers.contains(SyncTrigger::Manual));
        fixture
            .repository
            .schedule_sync_operation(ScheduleSyncInput {
                operation_id: OperationId::new(),
                account_id: fixture.account_id,
                scope: "inbox".to_owned(),
                trigger: SyncTrigger::Manual,
                mode: SyncMode::Incremental,
                scheduled_at_ms: 11,
            })
            .expect("coalesce running trigger");
        fixture
            .repository
            .commit_sync_batch(SyncBatchInput {
                operation_id,
                lease_id: first_lease.id,
                cursor_key: SyncCursorKey {
                    account_id: fixture.account_id,
                    scope: "inbox".to_owned(),
                },
                mailboxes: Vec::new(),
                changes: Vec::new(),
                checkpoint: DurableCheckpoint::new(
                    OpaqueProviderCursor::from_json("{\"generation\":1}").expect("checkpoint"),
                ),
                committed_at_ms: 12,
            })
            .expect("commit with follow-up trigger");
        assert_eq!(
            fixture
                .repository
                .get_sync_operation(operation_id)
                .expect("operation")
                .expect("scheduled follow-up")
                .state,
            SyncState::Scheduled
        );
        let second_lease = OperationLease {
            id: LeaseId::new(),
            expires_at_ms: 200,
        };
        let follow_up = fixture
            .repository
            .claim_sync_operation(ClaimSyncOperationInput {
                operation_id,
                provider: Provider::Gmail,
                lease: second_lease,
                claimed_at_ms: 13,
            })
            .expect("claim follow-up")
            .expect("claimed follow-up");
        assert!(follow_up.triggers.contains(SyncTrigger::Manual));
        assert!(
            fixture
                .repository
                .request_sync_cancellation(operation_id, 14)
                .expect("cancel running")
        );
        assert_eq!(
            fixture
                .repository
                .get_sync_operation(operation_id)
                .expect("cancel query")
                .expect("cancelled operation")
                .state,
            SyncState::Cancelled
        );
        assert_eq!(
            fixture.repository.commit_sync_batch(SyncBatchInput {
                operation_id,
                lease_id: second_lease.id,
                cursor_key: SyncCursorKey {
                    account_id: fixture.account_id,
                    scope: "inbox".to_owned(),
                },
                mailboxes: Vec::new(),
                changes: Vec::new(),
                checkpoint: DurableCheckpoint::new(
                    OpaqueProviderCursor::from_json("{\"generation\":2}")
                        .expect("cancel checkpoint"),
                ),
                committed_at_ms: 15,
            }),
            Err(RepositoryError::ConstraintViolation)
        );

        let second_scope = OperationId::new();
        let third_scope = OperationId::new();
        for (id, scope) in [(second_scope, "archive"), (third_scope, "sent")] {
            fixture
                .repository
                .schedule_sync_operation(ScheduleSyncInput {
                    operation_id: id,
                    account_id: fixture.account_id,
                    scope: scope.to_owned(),
                    trigger: SyncTrigger::Manual,
                    mode: SyncMode::Incremental,
                    scheduled_at_ms: 20,
                })
                .expect("schedule scope");
        }
        let scope_lease = OperationLease {
            id: LeaseId::new(),
            expires_at_ms: 100,
        };
        assert!(
            fixture
                .repository
                .claim_sync_operation(ClaimSyncOperationInput {
                    operation_id: second_scope,
                    provider: Provider::Gmail,
                    lease: scope_lease,
                    claimed_at_ms: 20,
                })
                .expect("claim second scope")
                .is_some()
        );
        assert!(
            fixture
                .repository
                .claim_sync_operation(ClaimSyncOperationInput {
                    operation_id: third_scope,
                    provider: Provider::Gmail,
                    lease: OperationLease {
                        id: LeaseId::new(),
                        expires_at_ms: 100,
                    },
                    claimed_at_ms: 20,
                })
                .expect("claim third scope")
                .is_none()
        );

        let other_account = AccountId::new();
        fixture
            .repository
            .create_account(AccountCreateInput {
                id: other_account,
                provider: Provider::Outlook,
                email: "other@example.test".to_owned(),
                display_name: None,
                credential_ref: CredentialRef::new("other-credential"),
                auth_state: AccountAuthState::Connected,
                enabled: true,
                created_at_ms: 20,
            })
            .expect("other account");
        let other_operation = OperationId::new();
        fixture
            .repository
            .schedule_sync_operation(ScheduleSyncInput {
                operation_id: other_operation,
                account_id: other_account,
                scope: "inbox".to_owned(),
                trigger: SyncTrigger::Manual,
                mode: SyncMode::Incremental,
                scheduled_at_ms: 20,
            })
            .expect("schedule other account");
        assert!(
            fixture
                .repository
                .claim_sync_operation(ClaimSyncOperationInput {
                    operation_id: other_operation,
                    provider: Provider::Outlook,
                    lease: OperationLease {
                        id: LeaseId::new(),
                        expires_at_ms: 100,
                    },
                    claimed_at_ms: 20,
                })
                .expect("claim other account")
                .is_some()
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn restart_clock_rollback_reclaims_sync_and_desired_read_backoff() {
        let fixture = fixture();
        let remote = remote_message(fixture.account_id, "remote-inbox", "rollback-read", false);
        commit_remote_changes(
            &fixture,
            "remote-inbox",
            vec![RemoteChange::Upsert(Box::new(remote))],
            9_000,
        );
        let message_id = fixture
            .repository
            .store
            .with_connection(|connection| {
                let raw: String = connection
                    .query_row(
                        "SELECT message_id FROM remote_message_ids
                         WHERE provider_mailbox_id='remote-inbox'
                           AND provider_message_id='rollback-read'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(|error| crate::StorageError::from_sql(&error))?;
                super::parse_id(&raw)
            })
            .expect("message id");
        let mutation = fixture
            .repository
            .set_message_read(MessageReadStateInput {
                message_id,
                read: true,
                updated_at_ms: 10_000,
            })
            .expect("desired read");
        let read_lease = OperationLease {
            id: LeaseId::new(),
            expires_at_ms: 70_000,
        };
        fixture
            .repository
            .claim_desired_read_mutation(ClaimDesiredReadMutationInput {
                key: mutation.key.clone(),
                generation: mutation.generation,
                lease: read_lease,
                claimed_at_ms: 10_000,
            })
            .expect("claim desired read")
            .expect("claimed desired read");
        assert!(
            fixture
                .repository
                .transition_desired_read_mutation(TransitionDesiredReadMutationInput {
                    key: mutation.key.clone(),
                    generation: mutation.generation,
                    lease_id: read_lease.id,
                    state: DesiredReadMutationState::WaitingBackoff,
                    release_lease: true,
                    attempt_count: 1,
                    next_attempt_at_ms: Some(12_345),
                    safe_error_code: None,
                    updated_at_ms: 10_000,
                })
                .expect("persist read backoff")
        );

        let operation_id = OperationId::new();
        fixture
            .repository
            .schedule_sync_operation(ScheduleSyncInput {
                operation_id,
                account_id: fixture.account_id,
                scope: "inbox".to_owned(),
                trigger: SyncTrigger::Manual,
                mode: SyncMode::Incremental,
                scheduled_at_ms: 10_000,
            })
            .expect("schedule sync");
        let sync_lease = OperationLease {
            id: LeaseId::new(),
            expires_at_ms: 70_000,
        };
        fixture
            .repository
            .claim_sync_operation(ClaimSyncOperationInput {
                operation_id,
                provider: Provider::Gmail,
                lease: sync_lease,
                claimed_at_ms: 10_000,
            })
            .expect("claim sync")
            .expect("claimed sync");
        assert!(
            fixture
                .repository
                .transition_sync_operation(TransitionSyncOperationInput {
                    operation_id,
                    lease_id: sync_lease.id,
                    mode: None,
                    state: SyncState::WaitingBackoff,
                    attempt_count: 1,
                    next_attempt_at_ms: Some(12_345),
                    safe_error_code: None,
                    updated_at_ms: 10_000,
                })
                .expect("persist sync backoff")
        );

        let restarted = SqlCipherRepository::initialize(
            &fixture.path,
            Arc::new(fixture.credentials.clone()) as Arc<dyn CredentialStore>,
        )
        .expect("restart repository");
        assert_eq!(
            restarted
                .list_runnable_sync_operations(Provider::Gmail, 5_000, 10)
                .expect("rollback sync due")
                .len(),
            1
        );
        let reclaimed_sync = restarted
            .claim_sync_operation(ClaimSyncOperationInput {
                operation_id,
                provider: Provider::Gmail,
                lease: OperationLease {
                    id: LeaseId::new(),
                    expires_at_ms: 65_000,
                },
                claimed_at_ms: 5_000,
            })
            .expect("reclaim sync")
            .expect("sync reclaimed after rollback");
        assert_eq!(reclaimed_sync.updated_at_ms, 10_000);
        assert_eq!(
            reclaimed_sync.lease.expect("sync lease").expires_at_ms,
            70_000
        );
        assert!(reclaimed_sync.updated_at_ms >= reclaimed_sync.created_at_ms);

        assert_eq!(
            restarted
                .list_due_desired_read_mutations(fixture.account_id, 5_000, 10)
                .expect("rollback read due")
                .len(),
            1
        );
        let reclaimed_read = restarted
            .claim_desired_read_mutation(ClaimDesiredReadMutationInput {
                key: mutation.key,
                generation: mutation.generation,
                lease: OperationLease {
                    id: LeaseId::new(),
                    expires_at_ms: 65_000,
                },
                claimed_at_ms: 5_000,
            })
            .expect("reclaim read")
            .expect("read reclaimed after rollback");
        assert_eq!(reclaimed_read.updated_at_ms, 10_000);
        assert_eq!(
            reclaimed_read.lease.expect("read lease").expires_at_ms,
            70_000
        );
        assert!(reclaimed_read.updated_at_ms >= reclaimed_read.created_at_ms);
    }

    #[test]
    fn offline_hint_fences_work_and_only_connectivity_restoration_resumes_it() {
        let fixture = fixture();
        let operation_id = OperationId::new();
        fixture
            .repository
            .schedule_sync_operation(ScheduleSyncInput {
                operation_id,
                account_id: fixture.account_id,
                scope: "inbox".to_owned(),
                trigger: SyncTrigger::Startup,
                mode: SyncMode::Initial(InitialSyncLimit::new(500).expect("valid limit")),
                scheduled_at_ms: 10,
            })
            .expect("schedule startup");
        assert_eq!(
            fixture
                .repository
                .mark_account_offline(fixture.account_id, 11)
                .expect("mark offline"),
            1
        );
        fixture
            .repository
            .schedule_sync_operation(ScheduleSyncInput {
                operation_id: OperationId::new(),
                account_id: fixture.account_id,
                scope: "inbox".to_owned(),
                trigger: SyncTrigger::Manual,
                mode: SyncMode::Incremental,
                scheduled_at_ms: 12,
            })
            .expect("coalesce manual while offline");
        assert_eq!(
            fixture
                .repository
                .get_sync_operation(operation_id)
                .expect("offline status")
                .expect("operation")
                .state,
            SyncState::Offline
        );
        assert!(
            fixture
                .repository
                .list_runnable_sync_operations(Provider::Gmail, 12, 10)
                .expect("offline runnable query")
                .is_empty()
        );
        fixture
            .repository
            .schedule_sync_operation(ScheduleSyncInput {
                operation_id: OperationId::new(),
                account_id: fixture.account_id,
                scope: "inbox".to_owned(),
                trigger: SyncTrigger::ConnectivityRestored,
                mode: SyncMode::Incremental,
                scheduled_at_ms: 13,
            })
            .expect("restore connectivity");
        assert_eq!(
            fixture
                .repository
                .get_sync_operation(operation_id)
                .expect("restored status")
                .expect("operation")
                .state,
            SyncState::Scheduled
        );
    }

    #[test]
    fn mailbox_scoped_identity_survives_replay_gone_and_reappearance() {
        let fixture = fixture();
        let inbox = remote_message(fixture.account_id, "remote-inbox", "same-id", false);
        let archive = remote_message(fixture.account_id, "remote-archive", "same-id", false);
        commit_remote_changes(
            &fixture,
            "remote-inbox",
            vec![RemoteChange::Upsert(Box::new(inbox.clone()))],
            10,
        );
        commit_remote_changes(
            &fixture,
            "remote-archive",
            vec![RemoteChange::Upsert(Box::new(archive))],
            11,
        );
        let (inbox_id, archive_id): (String, String) = fixture
            .repository
            .store
            .with_connection(|connection| {
                let inbox_id = connection
                    .query_row(
                        "SELECT message_id FROM remote_message_ids WHERE provider_mailbox_id='remote-inbox'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(|error| crate::StorageError::from_sql(&error))?;
                let archive_id = connection
                    .query_row(
                        "SELECT message_id FROM remote_message_ids WHERE provider_mailbox_id='remote-archive'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(|error| crate::StorageError::from_sql(&error))?;
                Ok((inbox_id, archive_id))
            })
            .expect("mapping ids");
        assert_ne!(inbox_id, archive_id);

        commit_remote_changes(
            &fixture,
            "remote-inbox",
            vec![RemoteChange::Upsert(Box::new(inbox.clone()))],
            12,
        );
        commit_remote_changes(
            &fixture,
            "remote-inbox",
            vec![RemoteChange::Gone(inbox.key.clone())],
            13,
        );
        let retained: (i64, i64) = fixture
            .repository
            .store
            .with_connection(|connection| {
                Ok((
                    connection
                        .query_row(
                            "SELECT count(*) FROM remote_message_ids WHERE message_id=?1",
                            [&inbox_id],
                            |row| row.get(0),
                        )
                        .map_err(|error| crate::StorageError::from_sql(&error))?,
                    connection
                        .query_row(
                            "SELECT count(*) FROM messages WHERE id=?1",
                            [&inbox_id],
                            |row| row.get(0),
                        )
                        .map_err(|error| crate::StorageError::from_sql(&error))?,
                ))
            })
            .expect("gone state");
        assert_eq!(retained, (1, 0));
        commit_remote_changes(
            &fixture,
            "remote-inbox",
            vec![RemoteChange::Upsert(Box::new(inbox))],
            14,
        );
        let reappeared: i64 = fixture
            .repository
            .store
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT count(*) FROM messages WHERE id=?1",
                        [&inbox_id],
                        |row| row.get(0),
                    )
                    .map_err(|error| crate::StorageError::from_sql(&error))
            })
            .expect("reappeared message");
        assert_eq!(reappeared, 1);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn desired_read_generations_and_expired_leases_are_recoverable() {
        let fixture = fixture();
        let remote = remote_message(fixture.account_id, "remote-inbox", "read-id", false);
        commit_remote_changes(
            &fixture,
            "remote-inbox",
            vec![RemoteChange::Upsert(Box::new(remote.clone()))],
            10,
        );
        let message_id: MessageId = fixture
            .repository
            .store
            .with_connection(|connection| {
                let raw: String = connection
                    .query_row(
                        "SELECT message_id FROM remote_message_ids WHERE provider_mailbox_id='remote-inbox' AND provider_message_id='read-id'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(|error| crate::StorageError::from_sql(&error))?;
                super::parse_id(&raw)
            })
            .expect("message id");
        let first = fixture
            .repository
            .set_message_read(MessageReadStateInput {
                message_id,
                read: true,
                updated_at_ms: 11,
            })
            .expect("first intent");
        let first_lease = OperationLease {
            id: LeaseId::new(),
            expires_at_ms: 20,
        };
        fixture
            .repository
            .claim_desired_read_mutation(ClaimDesiredReadMutationInput {
                key: first.key.clone(),
                generation: first.generation,
                lease: first_lease,
                claimed_at_ms: 12,
            })
            .expect("claim first intent")
            .expect("claimed first intent");
        let second = fixture
            .repository
            .set_message_read(MessageReadStateInput {
                message_id,
                read: false,
                updated_at_ms: 13,
            })
            .expect("second intent");
        assert!(second.generation > first.generation);
        let third = fixture
            .repository
            .set_message_read(MessageReadStateInput {
                message_id,
                read: true,
                updated_at_ms: 14,
            })
            .expect("third intent");
        assert!(third.generation > second.generation);
        assert!(
            !fixture
                .repository
                .complete_desired_read_mutation(CompleteDesiredReadMutationInput {
                    key: first.key,
                    generation: first.generation,
                    lease_id: first_lease.id,
                    provider_read: true,
                    provider_revision: None,
                    completed_at_ms: 15,
                })
                .expect("stale completion")
        );
        commit_remote_changes(
            &fixture,
            "remote-inbox",
            vec![RemoteChange::ReadState {
                key: third.key.clone(),
                read: true,
                revision: Some(ProviderRevision::new("revision-stale-true")),
            }],
            16,
        );
        let after_stale_observation = fixture
            .repository
            .list_due_desired_read_mutations(fixture.account_id, 16, 10)
            .expect("intent survives stale sync observation");
        assert_eq!(after_stale_observation.len(), 1);
        assert_eq!(after_stale_observation[0].generation, third.generation);
        let mut stale_upsert = remote.clone();
        stale_upsert.read = true;
        stale_upsert.provider_revision = Some(ProviderRevision::new("revision-stale-true"));
        commit_remote_changes(
            &fixture,
            "remote-inbox",
            vec![RemoteChange::Upsert(Box::new(stale_upsert))],
            17,
        );
        let after_stale_same_value = fixture
            .repository
            .list_due_desired_read_mutations(fixture.account_id, 17, 10)
            .expect("same-value stale upsert cannot acknowledge a newer generation");
        assert_eq!(after_stale_same_value.len(), 1);
        assert_eq!(after_stale_same_value[0].generation, third.generation);
        let third_lease = OperationLease {
            id: LeaseId::new(),
            expires_at_ms: 30,
        };
        fixture
            .repository
            .claim_desired_read_mutation(ClaimDesiredReadMutationInput {
                key: third.key.clone(),
                generation: third.generation,
                lease: third_lease,
                claimed_at_ms: 18,
            })
            .expect("claim third intent")
            .expect("claimed third intent");
        assert!(
            fixture
                .repository
                .complete_desired_read_mutation(CompleteDesiredReadMutationInput {
                    key: third.key,
                    generation: third.generation,
                    lease_id: third_lease.id,
                    provider_read: true,
                    provider_revision: Some(ProviderRevision::new("revision-true-new")),
                    completed_at_ms: 19,
                })
                .expect("generation-matched completion")
        );
        assert!(
            fixture
                .repository
                .list_due_desired_read_mutations(fixture.account_id, 19, 10)
                .expect("completed intent cleared")
                .is_empty()
        );
        let fourth = fixture
            .repository
            .set_message_read(MessageReadStateInput {
                message_id,
                read: false,
                updated_at_ms: 19,
            })
            .expect("fourth intent");
        assert!(fourth.generation > third.generation);
        let second_lease = OperationLease {
            id: LeaseId::new(),
            expires_at_ms: 21,
        };
        fixture
            .repository
            .claim_desired_read_mutation(ClaimDesiredReadMutationInput {
                key: fourth.key.clone(),
                generation: fourth.generation,
                lease: second_lease,
                claimed_at_ms: 20,
            })
            .expect("claim fourth intent")
            .expect("claimed fourth intent");
        let recovered = fixture
            .repository
            .recover_expired_leases(22)
            .expect("recover expired lease");
        assert_eq!(recovered.read_mutations_recovered, 1);
        let due = fixture
            .repository
            .list_due_desired_read_mutations(fixture.account_id, 22, 10)
            .expect("due mutations");
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].generation, fourth.generation);
        assert!(!due[0].desired_read);
    }

    #[test]
    fn offline_review_is_revision_bound_and_consumed_explicitly() {
        let fixture = fixture();
        let draft_id = DraftId::new();
        let draft = DraftSaveInput {
            id: draft_id,
            account_id: fixture.account_id,
            to: Vec::new(),
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: "Offline".to_owned(),
            plain_body: "Body".to_owned(),
            html_body: None,
            in_reply_to_message_id: None,
            attachments: Vec::new(),
            expected_revision: None,
            updated_at_ms: 2,
        };
        let retained = fixture
            .repository
            .save_draft_for_offline_review(OfflineDraftReviewInput {
                draft: draft.clone(),
                reviewed_at_ms: 2,
            })
            .expect("offline review");
        assert_eq!(retained.review.draft_revision, 1);
        assert_eq!(
            fixture
                .repository
                .list_send_confirmation_required(Some(fixture.account_id))
                .expect("confirmation list")
                .len(),
            1
        );
        let mut edited = draft;
        edited.expected_revision = Some(1);
        edited.updated_at_ms = 3;
        fixture.repository.save_draft(edited).expect("edit draft");
        assert!(
            fixture
                .repository
                .list_send_confirmation_required(Some(fixture.account_id))
                .expect("stale marker filtered")
                .is_empty()
        );
        assert!(
            !fixture
                .repository
                .consume_draft_send_review(DraftSendReviewKey {
                    draft_id,
                    draft_revision: 1,
                })
                .expect("consume stale marker")
        );
    }

    #[test]
    fn cleanup_resumes_after_credential_and_attachment_failures() {
        let mut fixture = fixture();
        fixture
            .credentials
            .set_fail_delete(true)
            .expect("inject delete failure");
        assert_eq!(
            fixture.repository.delete_account_local(fixture.account_id),
            Err(RepositoryError::CleanupPending)
        );
        assert!(
            fixture
                .repository
                .list_accounts()
                .expect("visible account list")
                .is_empty()
        );
        assert!(
            fixture
                .repository
                .get_account(fixture.account_id)
                .expect("visible account read")
                .is_none()
        );
        fixture
            .credentials
            .set_fail_delete(false)
            .expect("clear failure");
        drop(fixture.repository);
        fixture.repository = SqlCipherRepository::initialize(
            &fixture.path,
            Arc::new(fixture.credentials.clone()) as Arc<dyn CredentialStore>,
        )
        .expect("resume credential cleanup");
        assert!(
            fixture
                .repository
                .get_account(fixture.account_id)
                .expect("account read")
                .is_none()
        );
    }

    #[test]
    fn attachment_cleanup_plan_survives_restart() {
        let mut fixture = fixture();
        let cache_key = "blocked-cache";
        let blocked_directory = fixture.directory.path().join("attachments").join(cache_key);
        std::fs::create_dir_all(&blocked_directory).expect("create blocked cache directory");
        std::fs::write(blocked_directory.join("child"), b"pending").expect("create blocking child");
        let mut input = message(
            MessageId::new(),
            fixture.account_id,
            fixture.mailbox_id,
            "with-cache",
            "含附件",
            8,
        );
        input.attachments.push(AttachmentInput {
            id: AttachmentId::new(),
            provider_part_id: Some("part-1".to_owned()),
            file_name: Some("file.bin".to_owned()),
            media_type: "application/octet-stream".to_owned(),
            size_bytes: Some(7),
            content_id: None,
            inline: false,
            cache_key: Some(cache_key.to_owned()),
            checksum_sha256: None,
        });
        fixture
            .repository
            .upsert_message(input)
            .expect("message with cache");
        assert_eq!(
            fixture.repository.delete_account_local(fixture.account_id),
            Err(RepositoryError::CleanupPending)
        );
        assert!(
            fixture
                .repository
                .get_account(fixture.account_id)
                .expect("account read")
                .is_none()
        );
        drop(fixture.repository);
        std::fs::remove_file(blocked_directory.join("child")).expect("remove blocker");
        fixture.repository = SqlCipherRepository::initialize(
            &fixture.path,
            Arc::new(fixture.credentials.clone()) as Arc<dyn CredentialStore>,
        )
        .expect("resume attachment cleanup");
        assert!(!blocked_directory.exists());
        let cleanup_count = fixture
            .repository
            .store
            .with_connection(|connection| {
                connection
                    .query_row("SELECT count(*) FROM account_cleanup", [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .map_err(|error| crate::StorageError::from_sql(&error))
            })
            .expect("cleanup count");
        assert_eq!(cleanup_count, 0);
    }

    #[test]
    fn reused_cache_key_is_not_deleted_on_restart() {
        let mut fixture = fixture();
        let cache_key = "reused-cache";
        let cache_path = fixture.directory.path().join("attachments").join(cache_key);
        std::fs::write(&cache_path, b"live").expect("create cache file");
        let mut original = message(
            MessageId::new(),
            fixture.account_id,
            fixture.mailbox_id,
            "cache-original",
            "原附件",
            9,
        );
        original.attachments.push(AttachmentInput {
            id: AttachmentId::new(),
            provider_part_id: Some("old".to_owned()),
            file_name: Some("old.bin".to_owned()),
            media_type: "application/octet-stream".to_owned(),
            size_bytes: Some(4),
            content_id: None,
            inline: false,
            cache_key: Some(cache_key.to_owned()),
            checksum_sha256: None,
        });
        fixture
            .repository
            .upsert_message(original.clone())
            .expect("insert original cache reference");
        original.attachments.clear();
        original.updated_at_ms = 10;
        fixture
            .repository
            .upsert_message(original)
            .expect("queue removed cache reference");

        let live_message_id = MessageId::new();
        let mut reused = message(
            live_message_id,
            fixture.account_id,
            fixture.mailbox_id,
            "cache-reused",
            "复用附件",
            11,
        );
        reused.attachments.push(AttachmentInput {
            id: AttachmentId::new(),
            provider_part_id: Some("new".to_owned()),
            file_name: Some("new.bin".to_owned()),
            media_type: "application/octet-stream".to_owned(),
            size_bytes: Some(4),
            content_id: None,
            inline: false,
            cache_key: Some(cache_key.to_owned()),
            checksum_sha256: None,
        });
        fixture
            .repository
            .upsert_message(reused)
            .expect("reuse queued key atomically");
        drop(fixture.repository);
        fixture.repository = SqlCipherRepository::initialize(
            &fixture.path,
            Arc::new(fixture.credentials.clone()) as Arc<dyn CredentialStore>,
        )
        .expect("reopen with reused cache key");
        assert!(cache_path.exists());
        assert_eq!(
            fixture
                .repository
                .get_message(live_message_id)
                .expect("message read")
                .expect("live message")
                .attachments
                .len(),
            1
        );
    }

    #[test]
    fn account_cleanup_cache_keys_cannot_be_reused() {
        let fixture = fixture();
        let protected_key = "protected-cache";
        fixture
            .repository
            .store
            .with_connection(|connection| {
                connection
                    .execute(
                        "INSERT INTO account_cleanup(
                            account_id, credential_ref, attachment_cache_keys_json, stage, updated_at_ms
                         ) VALUES (?1, 'cleanup-ref', ?2, 'attachments', 1)",
                        rusqlite::params![
                            AccountId::new().to_string(),
                            serde_json::json!([protected_key]).to_string(),
                        ],
                    )
                    .map_err(|error| crate::StorageError::from_sql(&error))?;
                Ok(())
            })
            .expect("seed cleanup plan");
        let mut input = message(
            MessageId::new(),
            fixture.account_id,
            fixture.mailbox_id,
            "protected-reuse",
            "禁止复用",
            12,
        );
        input.attachments.push(AttachmentInput {
            id: AttachmentId::new(),
            provider_part_id: Some("protected".to_owned()),
            file_name: None,
            media_type: "application/octet-stream".to_owned(),
            size_bytes: Some(0),
            content_id: None,
            inline: false,
            cache_key: Some(protected_key.to_owned()),
            checksum_sha256: None,
        });
        assert_eq!(
            fixture.repository.upsert_message(input),
            Err(RepositoryError::ConstraintViolation)
        );
    }

    #[test]
    fn cache_cleanup_rejects_path_escape_and_treats_absent_files_as_idempotent() {
        let fixture = fixture();
        let absolute_key = fixture
            .directory
            .path()
            .join("outside-cache")
            .to_string_lossy()
            .into_owned();
        for (index, invalid_key) in [
            "../escape".to_owned(),
            "nested/child".to_owned(),
            absolute_key,
        ]
        .into_iter()
        .enumerate()
        {
            let mut invalid = message(
                MessageId::new(),
                fixture.account_id,
                fixture.mailbox_id,
                &format!("invalid-cache-{index}"),
                "非法缓存路径",
                13,
            );
            invalid.attachments.push(AttachmentInput {
                id: AttachmentId::new(),
                provider_part_id: Some(format!("invalid-{index}")),
                file_name: None,
                media_type: "application/octet-stream".to_owned(),
                size_bytes: Some(0),
                content_id: None,
                inline: false,
                cache_key: Some(invalid_key),
                checksum_sha256: None,
            });
            assert_eq!(
                fixture.repository.upsert_message(invalid),
                Err(RepositoryError::ConstraintViolation)
            );
        }

        let cache_key = "absent-cache";
        let mut input = message(
            MessageId::new(),
            fixture.account_id,
            fixture.mailbox_id,
            "absent-cache",
            "缺失缓存文件",
            14,
        );
        input.attachments.push(AttachmentInput {
            id: AttachmentId::new(),
            provider_part_id: Some("absent".to_owned()),
            file_name: None,
            media_type: "application/octet-stream".to_owned(),
            size_bytes: Some(0),
            content_id: None,
            inline: false,
            cache_key: Some(cache_key.to_owned()),
            checksum_sha256: None,
        });
        fixture
            .repository
            .upsert_message(input)
            .expect("store absent cache reference");
        assert!(
            fixture
                .repository
                .delete_account_local(fixture.account_id)
                .expect("delete with absent cache file")
                .deleted
        );
        assert!(
            !fixture
                .repository
                .delete_account_local(fixture.account_id)
                .expect("repeat account deletion")
                .deleted
        );
    }

    #[test]
    fn schema_rejects_cross_account_mailboxes_and_database_key_reference() {
        let fixture = fixture();
        let bad = fixture.repository.create_account(AccountCreateInput {
            id: AccountId::new(),
            provider: Provider::Outlook,
            email: "other@example.test".to_owned(),
            display_name: None,
            credential_ref: CredentialRef::new(crate::credentials::DATABASE_KEY_REF),
            auth_state: AccountAuthState::Connected,
            enabled: true,
            created_at_ms: 2,
        });
        assert_eq!(bad, Err(RepositoryError::ConstraintViolation));

        let other_id = AccountId::new();
        fixture
            .repository
            .create_account(AccountCreateInput {
                id: other_id,
                provider: Provider::Outlook,
                email: "other@example.test".to_owned(),
                display_name: None,
                credential_ref: CredentialRef::new("other-credential"),
                auth_state: AccountAuthState::Connected,
                enabled: true,
                created_at_ms: 2,
            })
            .expect("other account");
        let raw_cross_account = fixture.repository.store.with_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO messages(
                        id, account_id, mailbox_id, provider_message_id, subject, snippet,
                        is_read, direction, received_at_ms, parser_version, sanitizer_version,
                        created_at_ms, updated_at_ms
                     ) VALUES (?1, ?2, ?3, 'raw-cross', '', '', 0, 'incoming', 3, 1, 1, 3, 3)",
                    rusqlite::params![
                        MessageId::new().to_string(),
                        other_id.to_string(),
                        fixture.mailbox_id.to_string(),
                    ],
                )
                .map_err(|error| crate::StorageError::from_sql(&error))?;
            Ok(())
        });
        assert!(matches!(
            raw_cross_account,
            Err(crate::StorageError::Constraint)
        ));
        assert_eq!(
            fixture.repository.upsert_message(message(
                MessageId::new(),
                other_id,
                fixture.mailbox_id,
                "wrong-account",
                "错误账户",
                3,
            )),
            Err(RepositoryError::ConstraintViolation)
        );
    }
}
