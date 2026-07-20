use std::{
    fs,
    path::{Component, Path, PathBuf},
    str::FromStr,
    sync::Arc,
};

use rusqlite::{Connection, OptionalExtension, params};
use unimail_core::{
    Account, AccountAuthState, AccountCreateInput, AccountId, AddressRole, Attachment,
    AttachmentId, ClaimDesiredReadMutationInput, ClaimSyncOperationInput,
    CompleteDesiredReadMutationInput, CredentialRef, CredentialStore, DeleteAccountResult,
    DesiredReadMutation, DesiredReadMutationState, Draft, DraftAddress, DraftAttachmentInput,
    DraftId, DraftSaveInput, DraftSendReview, DraftSendReviewKey, DraftSendReviewReason,
    DraftSummary, DurableCheckpoint, InitialSyncLimit, LeaseRecoveryResult, Mailbox, MailboxId,
    MailboxRole, MailboxUpsertInput, MessageAddress, MessageAddressInput, MessageDetail,
    MessageDirection, MessageId, MessageListInput, MessagePage, MessagePageCursor,
    MessageReadStateInput, MessageSummary, MessageUpsertInput, MessageUpsertResult,
    MimeAddressRole, OfflineDraftReviewInput, OfflineDraftReviewResult, OpaqueProviderCursor,
    OperationId, OperationLease, Provider, ProviderRevision, ReadIntentGeneration, RemoteChange,
    RemoteMailbox, RemoteMessage, RemoteMessageKey, RepositoryError, RepositoryResult,
    SafeErrorCode, ScheduleSyncInput, SendConfirmationRequired, StorageRepository, StorageStatus,
    SyncBatchInput, SyncBatchResult, SyncCursor, SyncCursorKey, SyncMode, SyncOperation,
    SyncOperationSummary, SyncStage, SyncState, SyncTrigger, SyncTriggerSet,
    TransitionDesiredReadMutationInput, TransitionSyncOperationInput,
};

use crate::{ConnectionFactory, EncryptedStore, NativeCredentialStore, StorageError};

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
        fs::create_dir_all(&attachment_cache_root)
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
        self.store
            .with_connection(|connection| {
                let mut statement = connection
                    .prepare(
                        "SELECT m.id FROM email_fts f
                         JOIN messages m ON m.row_id = f.message_row_id
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

    /// Rebuilds the repository-owned full-text projection from normalized message rows.
    ///
    /// # Errors
    ///
    /// Returns a safe repository category and rolls back if rebuilding fails.
    pub fn rebuild_search_index(&self) -> RepositoryResult<()> {
        self.store
            .with_transaction(|transaction| {
                transaction
                    .execute_batch(
                        "DELETE FROM email_fts;
                         INSERT INTO email_fts(message_row_id, subject, body, sender)
                         SELECT m.row_id, m.subject,
                                coalesce(m.body_plain, '') || ' ' || coalesce(m.body_html, ''),
                                coalesce((SELECT coalesce(a.display_name, '') || ' ' || a.address
                                          FROM message_addresses a
                                          WHERE a.message_id=m.id AND a.role='from'
                                          ORDER BY a.position LIMIT 1), '')
                         FROM messages m;",
                    )
                    .map_err(|error| StorageError::from_sql(&error))
            })
            .map_err(map_storage_error)
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

    fn get_message(&self, message_id: MessageId) -> RepositoryResult<Option<MessageDetail>> {
        self.store
            .with_connection(|connection| get_message(connection, message_id))
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

    fn list_drafts(&self, account_id: AccountId) -> RepositoryResult<Vec<DraftSummary>> {
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
        now_ms: i64,
        limit: u32,
    ) -> RepositoryResult<Vec<SyncOperationSummary>> {
        self.store
            .with_connection(|connection| list_runnable_sync_operations(connection, now_ms, limit))
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
        input.html_body.as_deref().unwrap_or_default()
    );
    connection
        .execute(
            "INSERT INTO email_fts(message_row_id, subject, body, sender) VALUES (?1, ?2, ?3, ?4)",
            params![
                row_id,
                input.subject.as_deref().unwrap_or_default(),
                body,
                sender
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
    account_id: AccountId,
) -> Result<Vec<DraftSummary>, StorageError> {
    let mut statement = connection
        .prepare("SELECT id, subject, recipients_json, revision, updated_at_ms FROM drafts WHERE account_id=?1 ORDER BY updated_at_ms DESC, id DESC")
        .map_err(|error| StorageError::from_sql(&error))?;
    let rows = statement
        .query_map([account_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(|error| StorageError::from_sql(&error))?;
    rows.map(|row| {
        let (id, subject, recipients, revision, updated_at_ms) =
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
            account_id,
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
            "SELECT id, account_id, scope, trigger_bits, mode, mode_limit, stage, state,
                    attempt_count, next_attempt_at_ms, lease_id, lease_expires_at_ms,
                    cancel_generation, safe_error_code, created_at_ms, updated_at_ms,
                    started_at_ms, finished_at_ms
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
    now_ms: i64,
    limit: u32,
) -> Result<Vec<SyncOperationSummary>, StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT id, account_id, scope, trigger_bits, mode, mode_limit, stage, state,
                    attempt_count, next_attempt_at_ms, lease_id, lease_expires_at_ms,
                    cancel_generation, safe_error_code, created_at_ms, updated_at_ms,
                    started_at_ms, finished_at_ms
             FROM sync_operations
             WHERE state IN ('scheduled', 'waiting_backoff')
               AND (next_attempt_at_ms IS NULL OR next_attempt_at_ms<=?1 OR ?1<updated_at_ms)
               AND (lease_expires_at_ms IS NULL OR lease_expires_at_ms<=?1 OR ?1<updated_at_ms)
             ORDER BY coalesce(next_attempt_at_ms, scheduled_at_ms), scheduled_at_ms, id
             LIMIT ?2",
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    statement
        .query_map(params![now_ms, i64::from(limit.clamp(1, 100))], |row| {
            sync_operation_from_row(row).map(sync_summary)
        })
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
                let (inserted, acknowledged) =
                    upsert_remote_message(connection, message, committed_at_ms)?;
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
    updated_at_ms: i64,
) -> Result<(bool, bool), StorageError> {
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
                       ?13, ?14, ?15, 'incoming', ?16, ?17, 1, 1, ?18, ?19)
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
                message.sent_at_ms,
                message.received_at_ms,
                created_at_ms,
                updated_at_ms,
            ],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    replace_remote_message_children(connection, message_id, &message.key, message)?;
    refresh_message_fts(connection, message_id)?;
    Ok((inserted, false))
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
    connection
        .execute(
            "INSERT INTO email_fts(message_row_id, subject, body, sender)
             SELECT m.row_id, m.subject,
                    coalesce(m.body_plain, '') || ' ' || coalesce(m.body_html, ''),
                    coalesce((SELECT coalesce(a.display_name, '') || ' ' || a.address
                              FROM message_addresses a
                              WHERE a.message_id=m.id AND a.role IN ('from', 'sender')
                              ORDER BY CASE a.role WHEN 'from' THEN 0 ELSE 1 END, a.position LIMIT 1), '')
             FROM messages m WHERE m.id=?1",
            [message_id.to_string()],
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
        StorageError::Serialization => RepositoryError::InvalidData,
    }
}

#[cfg(test)]
mod tests {
    use std::{str::FromStr, sync::Arc};

    use secrecy::SecretBox;
    use tempfile::TempDir;
    use unimail_core::{
        AccountAuthState, AccountCreateInput, AccountId, AddressRole, AttachmentId,
        AttachmentInput, ClaimDesiredReadMutationInput, ClaimSyncOperationInput,
        CompleteDesiredReadMutationInput, CredentialRef, CredentialStore, DesiredReadMutationState,
        DraftAddress, DraftId, DraftSaveInput, DraftSendReviewKey, DurableCheckpoint,
        InitialSyncLimit, LeaseId, MailboxId, MailboxRole, MailboxUpsertInput, MessageAddressInput,
        MessageDirection, MessageId, MessageListInput, MessageReadStateInput, MessageUpsertInput,
        MimeAddress, MimeAddressEntry, MimeAddressRole, MimeBody, NormalizedMimeMessage,
        OfflineDraftReviewInput, OpaqueProviderCursor, OperationId, OperationLease, Provider,
        ProviderRevision, RemoteChange, RemoteMailbox, RemoteMailboxKey, RemoteMessage,
        RemoteMessageKey, RepositoryError, ScheduleSyncInput, StorageRepository, SyncBatchInput,
        SyncBatchResult, SyncCursorKey, SyncMode, SyncState, SyncTrigger,
        TransitionDesiredReadMutationInput, TransitionSyncOperationInput,
    };

    use super::SqlCipherRepository;
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
                .list_runnable_sync_operations(5_000, 10)
                .expect("rollback sync due")
                .len(),
            1
        );
        let reclaimed_sync = restarted
            .claim_sync_operation(ClaimSyncOperationInput {
                operation_id,
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
                .list_runnable_sync_operations(12, 10)
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
