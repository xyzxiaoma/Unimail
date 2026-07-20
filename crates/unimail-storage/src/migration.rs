use rusqlite_migration::{M, Migrations};

pub(crate) const SCHEMA_VERSION: u32 = 2;

pub(crate) fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(include_str!("../migrations/0001_initial.sql")),
        M::up(include_str!("../migrations/0002_sync_offline.sql")),
    ])
}

#[cfg(test)]
mod tests {
    use rusqlite::{Connection, OptionalExtension};
    use rusqlite_migration::{M, Migrations};

    use super::{SCHEMA_VERSION, migrations};

    #[test]
    fn migration_is_fresh_and_latest_to_latest_is_idempotent() {
        let mut connection = Connection::open_in_memory().expect("memory database");
        migrations()
            .to_latest(&mut connection)
            .expect("fresh migration");
        migrations()
            .to_latest(&mut connection)
            .expect("latest migration no-op");
        let version: u32 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("schema version");
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn failed_migration_rolls_back_all_statements() {
        let mut connection = Connection::open_in_memory().expect("memory database");
        let broken = Migrations::new(vec![M::up(
            "CREATE TABLE rollback_probe(id INTEGER); THIS IS NOT SQL;",
        )]);
        assert!(broken.to_latest(&mut connection).is_err());
        let table: Option<String> = connection
            .query_row(
                "SELECT name FROM sqlite_master WHERE name='rollback_probe'",
                [],
                |row| row.get(0),
            )
            .optional()
            .expect("schema query");
        assert!(table.is_none());
        let version: u32 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("schema version");
        assert_eq!(version, 0);
    }

    #[test]
    fn schema_never_declares_plaintext_secret_columns() {
        let schema = [
            include_str!("../migrations/0001_initial.sql"),
            include_str!("../migrations/0002_sync_offline.sql"),
        ]
        .concat()
        .to_ascii_lowercase();
        for forbidden in [
            "access_token",
            "refresh_token",
            "provider_password",
            "database_key",
            "authorization_code",
        ] {
            assert!(!schema.contains(forbidden), "forbidden column: {forbidden}");
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn v1_fixture_upgrades_without_losing_mail_data_or_fts() {
        let mut connection = Connection::open_in_memory().expect("memory database");
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .expect("foreign keys");
        Migrations::new(vec![M::up(include_str!("../migrations/0001_initial.sql"))])
            .to_latest(&mut connection)
            .expect("v1 migration");
        connection
            .execute_batch(
                "
                INSERT INTO accounts(id, provider, email, credential_ref, auth_state, created_at_ms, updated_at_ms)
                VALUES ('account-1', 'gmail', 'owner@example.com', 'credential-1', 'connected', 10, 10);
                INSERT INTO mailboxes(id, account_id, provider_mailbox_id, role, display_name, created_at_ms, updated_at_ms)
                VALUES ('mailbox-1', 'account-1', 'provider-inbox', 'inbox', 'Inbox', 10, 10);
                INSERT INTO messages(
                    row_id, id, account_id, mailbox_id, provider_message_id, provider_revision,
                    subject, snippet, body_plain, is_read, direction, received_at_ms,
                    parser_version, sanitizer_version, created_at_ms, updated_at_ms
                ) VALUES (
                    41, 'message-1', 'account-1', 'mailbox-1', 'remote-1', 'revision-1',
                    'Subject', 'Snippet', 'Body', 1, 'incoming', 20, 1, 1, 20, 20
                );
                INSERT INTO message_addresses(message_id, role, position, display_name, address)
                VALUES ('message-1', 'from', 0, 'Sender', 'sender@example.com');
                INSERT INTO attachments(id, message_id, provider_part_id, filename, media_type, size_bytes)
                VALUES ('attachment-1', 'message-1', 'part-1', 'file.txt', 'text/plain', 12);
                INSERT INTO drafts(
                    id, account_id, subject, body_plain, recipients_json, revision, created_at_ms, updated_at_ms
                ) VALUES ('draft-1', 'account-1', 'Draft', 'Draft body', '[]', 2, 30, 30);
                INSERT INTO sync_cursors(account_id, scope, cursor_json, updated_at_ms)
                VALUES ('account-1', 'inbox', '{\"token\":\"opaque\"}', 40);
                INSERT INTO pending_mutations(
                    id, account_id, message_id, mutation_type, payload_json, state,
                    attempt_count, created_at_ms, updated_at_ms
                ) VALUES (
                    'mutation-1', 'account-1', 'message-1', 'set_read',
                    '{\"desired_read\":false}', 'pending', 2, 50, 50
                );
                INSERT INTO sync_operations(
                    id, account_id, state, cursor_before_json, started_at_ms, created_at_ms
                ) VALUES ('operation-1', 'account-1', 'running', '{\"before\":1}', 45, 45);
                INSERT INTO email_fts(message_row_id, subject, body, sender)
                VALUES (41, 'Subject', 'Body', 'sender@example.com');
                ",
            )
            .expect("v1 fixture");

        migrations()
            .to_latest(&mut connection)
            .expect("upgrade to v2");

        let message: (i64, i64, i64, String) = connection
            .query_row(
                "SELECT row_id, remote_is_read, is_read, references_json FROM messages WHERE id = 'message-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("preserved message");
        assert_eq!(message, (41, 1, 1, "[]".to_owned()));
        let mapping: String = connection
            .query_row(
                "SELECT message_id FROM remote_message_ids WHERE account_id = 'account-1' AND provider_mailbox_id = 'provider-inbox' AND provider_message_id = 'remote-1'",
                [],
                |row| row.get(0),
            )
            .expect("remote mapping");
        assert_eq!(mapping, "message-1");
        let mutation: (i64, i64, String) = connection
            .query_row(
                "SELECT desired_read, intent_generation, state FROM pending_read_mutations WHERE message_id = 'message-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("typed mutation");
        assert_eq!(mutation, (0, 1, "pending".to_owned()));
        let cursor: (String, i64) = connection
            .query_row(
                "SELECT checkpoint_json, last_successful_at_ms FROM sync_cursors WHERE account_id = 'account-1' AND scope = 'inbox'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("cursor");
        assert_eq!(cursor, ("{\"token\":\"opaque\"}".to_owned(), 40));
        let fts_count: i64 = connection
            .query_row(
                "SELECT count(*) FROM email_fts WHERE message_row_id = 41",
                [],
                |row| row.get(0),
            )
            .expect("fts count");
        assert_eq!(fts_count, 1);
        let operation: (String, Option<String>, Option<i64>) = connection
            .query_row(
                "SELECT state, lease_id, lease_expires_at_ms FROM sync_operations WHERE id = 'operation-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("recovered V1 operation");
        assert_eq!(operation, ("scheduled".to_owned(), None, None));
        let integrity: String = connection
            .query_row("PRAGMA foreign_key_check", [], |row| row.get(0))
            .optional()
            .expect("foreign key check")
            .unwrap_or_default();
        assert!(integrity.is_empty());
    }

    #[test]
    fn v2_constraints_support_mailbox_identity_unknown_sizes_and_cascades() {
        let mut connection = Connection::open_in_memory().expect("memory database");
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .expect("foreign keys");
        migrations()
            .to_latest(&mut connection)
            .expect("fresh migration");
        connection
            .execute_batch(
                "
                INSERT INTO accounts(id, provider, email, credential_ref, auth_state, created_at_ms, updated_at_ms)
                VALUES ('account-1', 'gmail', 'owner@example.com', 'credential-1', 'connected', 1, 1);
                INSERT INTO mailboxes(id, account_id, provider_mailbox_id, role, display_name, created_at_ms, updated_at_ms)
                VALUES
                    ('mailbox-1', 'account-1', 'remote-inbox', 'inbox', 'Inbox', 1, 1),
                    ('mailbox-2', 'account-1', 'remote-other', 'other', 'Other', 1, 1);
                INSERT INTO messages(
                    id, account_id, mailbox_id, provider_message_id, references_json,
                    subject, snippet, remote_is_read, is_read, direction, received_at_ms,
                    parser_version, sanitizer_version, created_at_ms, updated_at_ms
                ) VALUES
                    ('message-1', 'account-1', 'mailbox-1', 'same-remote-id', '[]', '', '', 0, 0, 'incoming', 1, 1, 1, 1, 1),
                    ('message-2', 'account-1', 'mailbox-2', 'same-remote-id', '[]', '', '', 0, 0, 'incoming', 1, 1, 1, 1, 1);
                INSERT INTO remote_message_ids(account_id, provider_mailbox_id, provider_message_id, message_id, created_at_ms)
                VALUES
                    ('account-1', 'remote-inbox', 'same-remote-id', 'message-1', 1),
                    ('account-1', 'remote-other', 'same-remote-id', 'message-2', 1);
                INSERT INTO attachments(id, message_id, media_type, size_bytes)
                VALUES ('attachment-1', 'message-1', 'application/octet-stream', NULL);
                INSERT INTO drafts(id, account_id, recipients_json, revision, created_at_ms, updated_at_ms)
                VALUES ('draft-1', 'account-1', '[]', 1, 1, 1);
                INSERT INTO draft_send_reviews(draft_id, draft_revision, created_at_ms, updated_at_ms)
                VALUES ('draft-1', 1, 1, 1);
                ",
            )
            .expect("v2 fixture");

        let size: Option<i64> = connection
            .query_row(
                "SELECT size_bytes FROM attachments WHERE id = 'attachment-1'",
                [],
                |row| row.get(0),
            )
            .expect("nullable size");
        assert_eq!(size, None);
        connection
            .execute("DELETE FROM messages WHERE id = 'message-1'", [])
            .expect("delete live message");
        let retained_mapping: i64 = connection
            .query_row(
                "SELECT count(*) FROM remote_message_ids WHERE message_id = 'message-1'",
                [],
                |row| row.get(0),
            )
            .expect("retained mapping count");
        assert_eq!(retained_mapping, 1);
        connection
            .execute("DELETE FROM drafts WHERE id = 'draft-1'", [])
            .expect("delete draft");
        let review_count: i64 = connection
            .query_row("SELECT count(*) FROM draft_send_reviews", [], |row| {
                row.get(0)
            })
            .expect("review count");
        assert_eq!(review_count, 0);
        connection
            .execute("DELETE FROM accounts WHERE id = 'account-1'", [])
            .expect("delete account");
        let mapping_count: i64 = connection
            .query_row("SELECT count(*) FROM remote_message_ids", [], |row| {
                row.get(0)
            })
            .expect("mapping count");
        assert_eq!(mapping_count, 0);
    }
}
