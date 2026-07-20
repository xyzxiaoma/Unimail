DROP TRIGGER messages_email_fts_delete;
DROP INDEX messages_page_idx;
DROP INDEX messages_mailbox_page_idx;

ALTER TABLE messages RENAME TO messages_v1;

CREATE TABLE messages (
    row_id INTEGER PRIMARY KEY,
    id TEXT NOT NULL UNIQUE,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    mailbox_id TEXT NOT NULL,
    provider_message_id TEXT NOT NULL,
    provider_revision TEXT,
    thread_id TEXT,
    rfc_message_id TEXT,
    in_reply_to TEXT,
    references_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(references_json) AND json_type(references_json) = 'array'),
    subject TEXT NOT NULL DEFAULT '',
    snippet TEXT NOT NULL DEFAULT '',
    body_plain TEXT,
    body_html TEXT,
    remote_is_read INTEGER NOT NULL DEFAULT 0 CHECK (remote_is_read IN (0, 1)),
    is_read INTEGER NOT NULL DEFAULT 0 CHECK (is_read IN (0, 1)),
    direction TEXT NOT NULL CHECK (direction IN ('incoming', 'outgoing')),
    sent_at_ms INTEGER CHECK (sent_at_ms IS NULL OR sent_at_ms >= 0),
    received_at_ms INTEGER NOT NULL CHECK (received_at_ms >= 0),
    parser_version INTEGER NOT NULL CHECK (parser_version >= 1),
    sanitizer_version INTEGER NOT NULL CHECK (sanitizer_version >= 1),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
    UNIQUE(account_id, mailbox_id, provider_message_id),
    FOREIGN KEY(account_id, mailbox_id) REFERENCES mailboxes(account_id, id) ON DELETE CASCADE
);

INSERT INTO messages (
    row_id, id, account_id, mailbox_id, provider_message_id, provider_revision,
    thread_id, rfc_message_id, in_reply_to, references_json, subject, snippet,
    body_plain, body_html, remote_is_read, is_read, direction, sent_at_ms,
    received_at_ms, parser_version, sanitizer_version, created_at_ms, updated_at_ms
)
SELECT
    row_id, id, account_id, mailbox_id, provider_message_id, provider_revision,
    thread_id, rfc_message_id, NULL, '[]', subject, snippet, body_plain, body_html,
    is_read, is_read, direction, sent_at_ms, received_at_ms, parser_version,
    sanitizer_version, created_at_ms, updated_at_ms
FROM messages_v1;

ALTER TABLE message_addresses RENAME TO message_addresses_v1;

CREATE TABLE message_addresses (
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('from', 'sender', 'to', 'cc', 'bcc', 'reply_to')),
    position INTEGER NOT NULL CHECK (position >= 0),
    display_name TEXT,
    address TEXT NOT NULL,
    PRIMARY KEY(message_id, role, position)
);

INSERT INTO message_addresses(message_id, role, position, display_name, address)
SELECT message_id, role, position, display_name, address
FROM message_addresses_v1;

DROP TABLE message_addresses_v1;

DROP INDEX attachments_cache_key_unique;
ALTER TABLE attachments RENAME TO attachments_v1;

CREATE TABLE attachments (
    id TEXT PRIMARY KEY NOT NULL,
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    provider_part_id TEXT,
    filename TEXT,
    media_type TEXT NOT NULL,
    size_bytes INTEGER CHECK (size_bytes IS NULL OR size_bytes >= 0),
    content_id TEXT,
    is_inline INTEGER NOT NULL DEFAULT 0 CHECK (is_inline IN (0, 1)),
    cache_key TEXT,
    checksum_sha256 TEXT,
    UNIQUE(message_id, provider_part_id)
);

INSERT INTO attachments(
    id, message_id, provider_part_id, filename, media_type, size_bytes,
    content_id, is_inline, cache_key, checksum_sha256
)
SELECT
    id, message_id, provider_part_id, filename, media_type, size_bytes,
    content_id, is_inline, cache_key, checksum_sha256
FROM attachments_v1;

DROP TABLE attachments_v1;

CREATE UNIQUE INDEX attachments_cache_key_unique
ON attachments(cache_key) WHERE cache_key IS NOT NULL;

CREATE TABLE remote_message_ids (
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    provider_mailbox_id TEXT NOT NULL,
    provider_message_id TEXT NOT NULL,
    message_id TEXT NOT NULL UNIQUE,
    read_intent_generation INTEGER NOT NULL DEFAULT 0 CHECK (read_intent_generation >= 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    PRIMARY KEY(account_id, provider_mailbox_id, provider_message_id)
);

INSERT INTO remote_message_ids(
    account_id, provider_mailbox_id, provider_message_id, message_id, created_at_ms
)
SELECT
    messages.account_id, mailboxes.provider_mailbox_id,
    messages.provider_message_id, messages.id, messages.created_at_ms
FROM messages
JOIN mailboxes ON mailboxes.id = messages.mailbox_id
              AND mailboxes.account_id = messages.account_id;

ALTER TABLE pending_mutations RENAME TO pending_mutations_v1;

CREATE TABLE pending_read_mutations (
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    provider_mailbox_id TEXT NOT NULL,
    provider_message_id TEXT NOT NULL,
    message_id TEXT NOT NULL UNIQUE REFERENCES messages(id) ON DELETE CASCADE,
    desired_read INTEGER NOT NULL CHECK (desired_read IN (0, 1)),
    expected_provider_revision TEXT,
    intent_generation INTEGER NOT NULL CHECK (intent_generation >= 1),
    state TEXT NOT NULL CHECK (state IN ('pending', 'running', 'waiting_backoff', 'needs_auth', 'failed')),
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    next_attempt_at_ms INTEGER CHECK (next_attempt_at_ms IS NULL OR next_attempt_at_ms >= 0),
    lease_id TEXT,
    lease_expires_at_ms INTEGER CHECK (lease_expires_at_ms IS NULL OR lease_expires_at_ms >= 0),
    safe_error_code TEXT,
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
    PRIMARY KEY(account_id, provider_mailbox_id, provider_message_id),
    CHECK ((lease_id IS NULL) = (lease_expires_at_ms IS NULL)),
    FOREIGN KEY(account_id, provider_mailbox_id, provider_message_id)
        REFERENCES remote_message_ids(account_id, provider_mailbox_id, provider_message_id)
        ON DELETE CASCADE
);

INSERT INTO pending_read_mutations(
    account_id, provider_mailbox_id, provider_message_id, message_id,
    desired_read, expected_provider_revision, intent_generation, state,
    attempt_count, next_attempt_at_ms, lease_id, lease_expires_at_ms,
    safe_error_code, created_at_ms, updated_at_ms
)
SELECT
    old.account_id, mailbox.provider_mailbox_id, message.provider_message_id,
    message.id,
    CASE
        WHEN json_type(old.payload_json, '$.desired_read') IN ('true', 'false')
            THEN json_extract(old.payload_json, '$.desired_read')
        ELSE json_extract(old.payload_json, '$.read')
    END,
    message.provider_revision, 1,
    CASE old.state WHEN 'running' THEN 'pending' ELSE old.state END,
    old.attempt_count, old.next_attempt_at_ms, NULL, NULL, NULL,
    old.created_at_ms, old.updated_at_ms
FROM pending_mutations_v1 AS old
JOIN messages AS message ON message.id = old.message_id
JOIN mailboxes AS mailbox ON mailbox.id = message.mailbox_id
                         AND mailbox.account_id = message.account_id
WHERE old.mutation_type IN ('set_read', 'desired_read')
  AND (
      json_type(old.payload_json, '$.desired_read') IN ('true', 'false')
      OR json_type(old.payload_json, '$.read') IN ('true', 'false')
  );

DROP TABLE pending_mutations_v1;
DROP TABLE messages_v1;

CREATE INDEX messages_page_idx ON messages(account_id, received_at_ms DESC, id DESC);
CREATE INDEX messages_mailbox_page_idx ON messages(mailbox_id, received_at_ms DESC, id DESC);

CREATE TRIGGER messages_email_fts_delete
AFTER DELETE ON messages
BEGIN
    DELETE FROM email_fts WHERE message_row_id = OLD.row_id;
END;

ALTER TABLE sync_cursors RENAME TO sync_cursors_v1;

CREATE TABLE sync_cursors (
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    scope TEXT NOT NULL,
    checkpoint_json TEXT NOT NULL CHECK (json_valid(checkpoint_json)),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0),
    last_successful_at_ms INTEGER NOT NULL CHECK (last_successful_at_ms >= 0),
    PRIMARY KEY(account_id, scope)
);

INSERT INTO sync_cursors(
    account_id, scope, checkpoint_json, updated_at_ms, last_successful_at_ms
)
SELECT account_id, scope, cursor_json, updated_at_ms, updated_at_ms
FROM sync_cursors_v1;

DROP TABLE sync_cursors_v1;

ALTER TABLE sync_operations RENAME TO sync_operations_v1;

CREATE TABLE sync_operations (
    id TEXT PRIMARY KEY NOT NULL,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    scope TEXT NOT NULL,
    trigger_bits INTEGER NOT NULL CHECK (trigger_bits >= 0),
    mode TEXT NOT NULL CHECK (mode IN ('initial', 'incremental', 'cursor_reset')),
    mode_limit INTEGER CHECK (mode_limit IS NULL OR mode_limit BETWEEN 1 AND 500),
    stage TEXT CHECK (stage IS NULL OR stage IN ('load', 'fetch', 'commit', 'flush_read_mutations')),
    state TEXT NOT NULL CHECK (state IN (
        'scheduled', 'running', 'waiting_backoff', 'offline', 'needs_auth',
        'committed', 'failed', 'cancelled'
    )),
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    next_attempt_at_ms INTEGER CHECK (next_attempt_at_ms IS NULL OR next_attempt_at_ms >= 0),
    lease_id TEXT,
    lease_expires_at_ms INTEGER CHECK (lease_expires_at_ms IS NULL OR lease_expires_at_ms >= 0),
    cancel_generation INTEGER NOT NULL DEFAULT 0 CHECK (cancel_generation >= 0),
    cursor_before_json TEXT CHECK (cursor_before_json IS NULL OR json_valid(cursor_before_json)),
    cursor_after_json TEXT CHECK (cursor_after_json IS NULL OR json_valid(cursor_after_json)),
    safe_error_code TEXT,
    scheduled_at_ms INTEGER NOT NULL CHECK (scheduled_at_ms >= 0),
    started_at_ms INTEGER CHECK (started_at_ms IS NULL OR started_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
    finished_at_ms INTEGER CHECK (finished_at_ms IS NULL OR finished_at_ms >= 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    CHECK ((lease_id IS NULL) = (lease_expires_at_ms IS NULL)),
    CHECK ((mode = 'incremental' AND mode_limit IS NULL) OR
           (mode IN ('initial', 'cursor_reset') AND mode_limit IS NOT NULL))
);

INSERT INTO sync_operations(
    id, account_id, scope, trigger_bits, mode, mode_limit, stage, state, attempt_count,
    next_attempt_at_ms, lease_id, lease_expires_at_ms, cancel_generation,
    cursor_before_json, cursor_after_json, safe_error_code, scheduled_at_ms,
    started_at_ms, updated_at_ms, finished_at_ms, created_at_ms
)
SELECT
    id, account_id, 'inbox', 1, 'initial', 500,
    CASE WHEN state = 'running' THEN 'load' ELSE NULL END,
    state, 0, NULL, NULL, NULL, 0, cursor_before_json, cursor_after_json,
    safe_error_code, created_at_ms, started_at_ms,
    COALESCE(finished_at_ms, started_at_ms, created_at_ms), finished_at_ms, created_at_ms
FROM sync_operations_v1;

DROP TABLE sync_operations_v1;

-- V1 running rows have no durable lease metadata. Recover every one to scheduled so none can
-- become an unreclaimable running row after the V2 lease rules take effect.
UPDATE sync_operations
SET state = 'scheduled', stage = NULL, started_at_ms = NULL
WHERE state = 'running';

CREATE UNIQUE INDEX sync_operations_one_active_scope
ON sync_operations(account_id)
WHERE state = 'running';

CREATE INDEX sync_operations_due_idx
ON sync_operations(state, next_attempt_at_ms, scheduled_at_ms);

CREATE INDEX pending_read_mutations_due_idx
ON pending_read_mutations(state, next_attempt_at_ms, updated_at_ms);

CREATE TABLE draft_send_reviews (
    draft_id TEXT PRIMARY KEY NOT NULL REFERENCES drafts(id) ON DELETE CASCADE,
    draft_revision INTEGER NOT NULL CHECK (draft_revision >= 1),
    reason TEXT NOT NULL DEFAULT 'offline' CHECK (reason = 'offline'),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms)
);
