CREATE TABLE accounts (
    id TEXT PRIMARY KEY NOT NULL,
    provider TEXT NOT NULL CHECK (provider IN ('gmail', 'outlook', 'qq', 'netease')),
    email TEXT NOT NULL,
    display_name TEXT,
    credential_ref TEXT NOT NULL UNIQUE CHECK (credential_ref <> 'database-key-v1'),
    auth_state TEXT NOT NULL CHECK (auth_state IN ('connected', 'needs_auth', 'unavailable')),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    deleting INTEGER NOT NULL DEFAULT 0 CHECK (deleting IN (0, 1)),
    cleanup_state TEXT NOT NULL DEFAULT 'none' CHECK (cleanup_state IN ('none', 'credentials', 'database', 'attachments')),
    last_error_code TEXT,
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
    UNIQUE(provider, email)
);

CREATE TABLE mailboxes (
    id TEXT PRIMARY KEY NOT NULL,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    provider_mailbox_id TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('inbox', 'sent', 'other')),
    display_name TEXT NOT NULL,
    metadata_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(metadata_json)),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
    UNIQUE(account_id, provider_mailbox_id),
    UNIQUE(account_id, id)
);

CREATE TABLE messages (
    row_id INTEGER PRIMARY KEY,
    id TEXT NOT NULL UNIQUE,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    mailbox_id TEXT NOT NULL,
    provider_message_id TEXT NOT NULL,
    provider_revision TEXT,
    thread_id TEXT,
    rfc_message_id TEXT,
    subject TEXT NOT NULL DEFAULT '',
    snippet TEXT NOT NULL DEFAULT '',
    body_plain TEXT,
    body_html TEXT,
    is_read INTEGER NOT NULL DEFAULT 0 CHECK (is_read IN (0, 1)),
    direction TEXT NOT NULL CHECK (direction IN ('incoming', 'outgoing')),
    sent_at_ms INTEGER CHECK (sent_at_ms IS NULL OR sent_at_ms >= 0),
    received_at_ms INTEGER NOT NULL CHECK (received_at_ms >= 0),
    parser_version INTEGER NOT NULL CHECK (parser_version >= 1),
    sanitizer_version INTEGER NOT NULL CHECK (sanitizer_version >= 1),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
    UNIQUE(account_id, provider_message_id),
    FOREIGN KEY(account_id, mailbox_id) REFERENCES mailboxes(account_id, id) ON DELETE CASCADE
);

CREATE INDEX messages_page_idx ON messages(account_id, received_at_ms DESC, id DESC);
CREATE INDEX messages_mailbox_page_idx ON messages(mailbox_id, received_at_ms DESC, id DESC);

CREATE TABLE message_addresses (
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('from', 'to', 'cc', 'bcc', 'reply_to')),
    position INTEGER NOT NULL CHECK (position >= 0),
    display_name TEXT,
    address TEXT NOT NULL,
    PRIMARY KEY(message_id, role, position)
);

CREATE TABLE attachments (
    id TEXT PRIMARY KEY NOT NULL,
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    provider_part_id TEXT,
    filename TEXT,
    media_type TEXT NOT NULL,
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    content_id TEXT,
    is_inline INTEGER NOT NULL DEFAULT 0 CHECK (is_inline IN (0, 1)),
    cache_key TEXT,
    checksum_sha256 TEXT,
    UNIQUE(message_id, provider_part_id)
);

CREATE UNIQUE INDEX attachments_cache_key_unique
ON attachments(cache_key) WHERE cache_key IS NOT NULL;

CREATE TABLE drafts (
    id TEXT PRIMARY KEY NOT NULL,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    subject TEXT NOT NULL DEFAULT '',
    body_plain TEXT NOT NULL DEFAULT '',
    body_html TEXT,
    recipients_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(recipients_json)),
    in_reply_to TEXT,
    thread_id TEXT,
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms)
);

CREATE INDEX drafts_account_updated_idx ON drafts(account_id, updated_at_ms DESC, id DESC);

CREATE TABLE draft_attachments (
    -- local_ref identifies a user-selected draft source; it is not an owned cache path.
    id TEXT PRIMARY KEY NOT NULL,
    draft_id TEXT NOT NULL REFERENCES drafts(id) ON DELETE CASCADE,
    filename TEXT NOT NULL,
    media_type TEXT NOT NULL,
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    local_ref TEXT NOT NULL,
    position INTEGER NOT NULL CHECK (position >= 0),
    UNIQUE(draft_id, position)
);

CREATE TABLE sync_cursors (
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    scope TEXT NOT NULL,
    cursor_json TEXT NOT NULL CHECK (json_valid(cursor_json)),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0),
    PRIMARY KEY(account_id, scope)
);

CREATE TABLE sync_operations (
    id TEXT PRIMARY KEY NOT NULL,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    state TEXT NOT NULL CHECK (state IN ('scheduled', 'running', 'committed', 'failed', 'cancelled')),
    cursor_before_json TEXT CHECK (cursor_before_json IS NULL OR json_valid(cursor_before_json)),
    cursor_after_json TEXT CHECK (cursor_after_json IS NULL OR json_valid(cursor_after_json)),
    safe_error_code TEXT,
    started_at_ms INTEGER CHECK (started_at_ms IS NULL OR started_at_ms >= 0),
    finished_at_ms INTEGER CHECK (finished_at_ms IS NULL OR finished_at_ms >= 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0)
);

CREATE TABLE pending_mutations (
    id TEXT PRIMARY KEY NOT NULL,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    message_id TEXT REFERENCES messages(id) ON DELETE CASCADE,
    mutation_type TEXT NOT NULL,
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    state TEXT NOT NULL CHECK (state IN ('pending', 'running', 'failed')),
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    next_attempt_at_ms INTEGER CHECK (next_attempt_at_ms IS NULL OR next_attempt_at_ms >= 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms)
);

CREATE TABLE app_settings (
    key TEXT PRIMARY KEY NOT NULL,
    value_json TEXT NOT NULL CHECK (json_valid(value_json)),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0)
);

CREATE TABLE attachment_cleanup_queue (
    account_id TEXT NOT NULL,
    cache_key TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    PRIMARY KEY(account_id, cache_key)
);

CREATE TABLE account_cleanup (
    account_id TEXT PRIMARY KEY NOT NULL,
    credential_ref TEXT NOT NULL,
    attachment_cache_keys_json TEXT NOT NULL CHECK (json_valid(attachment_cache_keys_json)),
    stage TEXT NOT NULL CHECK (stage IN ('credentials', 'database', 'attachments')),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0)
);

CREATE VIRTUAL TABLE email_fts USING fts5(
    message_row_id UNINDEXED,
    subject,
    body,
    sender,
    tokenize = 'unicode61'
);

CREATE TRIGGER messages_email_fts_delete
AFTER DELETE ON messages
BEGIN
    DELETE FROM email_fts WHERE message_row_id = OLD.row_id;
END;
