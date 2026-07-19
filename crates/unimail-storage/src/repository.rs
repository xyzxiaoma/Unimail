use std::{
    fs,
    path::{Component, Path, PathBuf},
    str::FromStr,
    sync::Arc,
};

use rusqlite::{Connection, OptionalExtension, params};
use unimail_core::{
    Account, AccountAuthState, AccountCreateInput, AccountId, AddressRole, Attachment,
    CredentialRef, CredentialStore, DeleteAccountResult, Draft, DraftAddress, DraftAttachmentInput,
    DraftId, DraftSaveInput, DraftSummary, Mailbox, MailboxRole, MailboxUpsertInput,
    MessageAddress, MessageAddressInput, MessageDetail, MessageDirection, MessageId,
    MessageListInput, MessagePage, MessagePageCursor, MessageReadStateInput, MessageSummary,
    MessageUpsertInput, MessageUpsertResult, Provider, RepositoryError, RepositoryResult,
    StorageRepository, StorageStatus, SyncBatchInput, SyncBatchResult, SyncCursor, SyncCursorKey,
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

    fn set_message_read(&self, input: MessageReadStateInput) -> RepositoryResult<bool> {
        self.store
            .with_connection(|connection| {
                connection
                    .execute(
                        "UPDATE messages SET is_read = ?2, updated_at_ms = ?3 WHERE id = ?1",
                        params![
                            input.message_id.to_string(),
                            input.read,
                            input.updated_at_ms
                        ],
                    )
                    .map(|changed| changed > 0)
                    .map_err(|error| StorageError::from_sql(&error))
            })
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

    fn get_sync_cursor(&self, key: &SyncCursorKey) -> RepositoryResult<Option<SyncCursor>> {
        self.store
            .with_connection(|connection| get_sync_cursor(connection, key))
            .map_err(map_storage_error)
    }

    fn commit_sync_batch(&self, input: SyncBatchInput) -> RepositoryResult<SyncBatchResult> {
        self.store
            .with_transaction(|transaction| {
                transaction
                    .execute(
                        "INSERT INTO sync_operations(id, account_id, state, created_at_ms, started_at_ms)
                         VALUES (?1, ?2, 'running', ?3, ?3)",
                        params![
                            input.operation_id.to_string(),
                            input.cursor.key.account_id.to_string(),
                            input.committed_at_ms,
                        ],
                    )
                    .map_err(|error| StorageError::from_sql(&error))?;
                for mailbox in &input.mailboxes {
                    if mailbox.account_id != input.cursor.key.account_id {
                        return Err(StorageError::Constraint);
                    }
                    upsert_mailbox(transaction, mailbox)?;
                }
                let mut inserted_messages = 0_u32;
                let mut updated_messages = 0_u32;
                for message in &input.messages {
                    if message.account_id != input.cursor.key.account_id {
                        return Err(StorageError::Constraint);
                    }
                    if upsert_message(transaction, message)?.inserted {
                        inserted_messages += 1;
                    } else {
                        updated_messages += 1;
                    }
                }
                store_sync_cursor(transaction, &input.cursor)?;
                transaction
                    .execute(
                        "UPDATE sync_operations SET state = 'committed', finished_at_ms = ?2,
                                cursor_after_json = ?3 WHERE id = ?1",
                        params![
                            input.operation_id.to_string(),
                            input.committed_at_ms,
                            serde_json::to_string(&input.cursor.value)
                                .map_err(|_| StorageError::Serialization)?,
                        ],
                    )
                    .map_err(|error| StorageError::from_sql(&error))?;
                Ok(SyncBatchResult {
                    operation_id: input.operation_id,
                    inserted_messages,
                    updated_messages,
                })
            })
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
    let existing: Option<(String, i64)> = connection
        .query_row(
            "SELECT id, created_at_ms FROM mailboxes
             WHERE account_id = ?1 AND provider_mailbox_id = ?2",
            params![input.account_id.to_string(), input.provider_mailbox_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))?;
    let (id, created_at_ms) = match existing {
        Some((id, created)) => (parse_id(&id)?, created),
        None => (input.id, input.updated_at_ms),
    };
    connection
        .execute(
            "INSERT INTO mailboxes(
                id, account_id, provider_mailbox_id, role, display_name,
                created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(account_id, provider_mailbox_id) DO UPDATE SET
                role = excluded.role, display_name = excluded.display_name,
                updated_at_ms = excluded.updated_at_ms",
            params![
                id.to_string(),
                input.account_id.to_string(),
                input.provider_mailbox_id,
                mailbox_role_to_str(input.role),
                input.display_name,
                created_at_ms,
                input.updated_at_ms,
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
        updated_at_ms: input.updated_at_ms,
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
    let row: Option<(String, i64)> = connection
        .query_row(
            "SELECT cursor_json, updated_at_ms FROM sync_cursors WHERE account_id=?1 AND scope=?2",
            params![key.account_id.to_string(), key.scope],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| StorageError::from_sql(&error))?;
    row.map(|(encoded, updated_at_ms)| {
        Ok(SyncCursor {
            key: key.clone(),
            value: serde_json::from_str(&encoded).map_err(|_| StorageError::Serialization)?,
            updated_at_ms,
        })
    })
    .transpose()
}

fn store_sync_cursor(connection: &Connection, cursor: &SyncCursor) -> Result<(), StorageError> {
    let encoded = serde_json::to_string(&cursor.value).map_err(|_| StorageError::Serialization)?;
    connection
        .execute(
            "INSERT INTO sync_cursors(account_id, scope, cursor_json, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(account_id, scope) DO UPDATE SET
                cursor_json=excluded.cursor_json, updated_at_ms=excluded.updated_at_ms",
            params![
                cursor.key.account_id.to_string(),
                cursor.key.scope,
                encoded,
                cursor.updated_at_ms,
            ],
        )
        .map_err(|error| StorageError::from_sql(&error))?;
    Ok(())
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
        AddressRole::To => "to",
        AddressRole::Cc => "cc",
        AddressRole::Bcc => "bcc",
        AddressRole::ReplyTo => "reply_to",
    }
}

fn address_role_from_str(value: &str) -> Result<AddressRole, StorageError> {
    match value {
        "from" => Ok(AddressRole::From),
        "to" => Ok(AddressRole::To),
        "cc" => Ok(AddressRole::Cc),
        "bcc" => Ok(AddressRole::Bcc),
        "reply_to" => Ok(AddressRole::ReplyTo),
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
        AttachmentInput, CredentialRef, CredentialStore, DraftAddress, DraftId, DraftSaveInput,
        MailboxId, MailboxRole, MailboxUpsertInput, MessageAddressInput, MessageDirection,
        MessageId, MessageListInput, MessageUpsertInput, OperationId, Provider, RepositoryError,
        StorageRepository, SyncBatchInput, SyncCursor, SyncCursorKey,
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
    fn cursor_and_message_batch_commit_or_rollback_together() {
        let fixture = fixture();
        let key = SyncCursorKey {
            account_id: fixture.account_id,
            scope: "inbox".to_owned(),
        };
        let mut invalid = message(
            MessageId::new(),
            fixture.account_id,
            MailboxId::new(),
            "atomic",
            "原子性",
            5,
        );
        invalid.mailbox_id = MailboxId::new();
        let batch = SyncBatchInput {
            operation_id: OperationId::new(),
            mailboxes: Vec::new(),
            messages: vec![invalid],
            cursor: SyncCursor {
                key: key.clone(),
                value: "next".to_owned(),
                updated_at_ms: 5,
            },
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

        let valid = SyncBatchInput {
            operation_id: OperationId::new(),
            mailboxes: Vec::new(),
            messages: vec![message(
                MessageId::new(),
                fixture.account_id,
                fixture.mailbox_id,
                "atomic",
                "原子性",
                5,
            )],
            cursor: SyncCursor {
                key: key.clone(),
                value: "next".to_owned(),
                updated_at_ms: 5,
            },
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
                .value,
            "next"
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
            size_bytes: 7,
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
            size_bytes: 4,
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
            size_bytes: 4,
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
            size_bytes: 0,
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
                size_bytes: 0,
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
            size_bytes: 0,
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
