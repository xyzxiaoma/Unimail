CREATE TABLE outbound_attempts (
    id TEXT PRIMARY KEY NOT NULL,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    draft_id TEXT NOT NULL,
    draft_revision INTEGER NOT NULL CHECK (draft_revision >= 1),
    reply_source_message_id TEXT REFERENCES messages(id) ON DELETE SET NULL,
    provider_thread_id TEXT,
    original_provider_message_id TEXT,
    rfc_message_id TEXT NOT NULL,
    date_rfc2822 TEXT NOT NULL,
    exact_mime BLOB NOT NULL,
    envelope_from TEXT NOT NULL,
    envelope_recipients_json TEXT NOT NULL CHECK (json_valid(envelope_recipients_json)),
    sender_json TEXT NOT NULL CHECK (json_valid(sender_json)),
    recipients_json TEXT NOT NULL CHECK (json_valid(recipients_json)),
    subject TEXT NOT NULL DEFAULT '',
    body_plain TEXT NOT NULL DEFAULT '',
    state TEXT NOT NULL CHECK (state IN (
        'submitting', 'accepted_pending', 'reconciled', 'rejected', 'unknown_locked'
    )),
    send_blocked INTEGER NOT NULL DEFAULT 1 CHECK (send_blocked IN (0, 1)),
    provider_message_id TEXT,
    reconciled_message_id TEXT REFERENCES messages(id) ON DELETE SET NULL,
    safe_error_code TEXT CHECK (safe_error_code IS NULL OR safe_error_code IN (
        'recipient_rejected', 'authentication_required', 'provider_unavailable',
        'invalid_draft', 'internal'
    )),
    sent_refresh_count INTEGER NOT NULL DEFAULT 0 CHECK (sent_refresh_count >= 0),
    retry_authorized INTEGER NOT NULL DEFAULT 0 CHECK (retry_authorized IN (0, 1)),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
    UNIQUE(account_id, rfc_message_id),
    CHECK (
        (state IN ('submitting', 'unknown_locked') AND send_blocked = 1)
        OR (state IN ('accepted_pending', 'reconciled', 'rejected') AND send_blocked = 0)
        OR (state = 'unknown_locked' AND retry_authorized = 1 AND send_blocked = 0)
    ),
    CHECK (retry_authorized = 0 OR (state = 'unknown_locked' AND sent_refresh_count >= 1))
);

CREATE UNIQUE INDEX outbound_attempts_one_blocker_per_draft
ON outbound_attempts(draft_id) WHERE send_blocked = 1;

CREATE INDEX outbound_attempts_sent_idx
ON outbound_attempts(account_id, updated_at_ms DESC, id DESC)
WHERE state IN ('accepted_pending', 'reconciled', 'unknown_locked');

CREATE INDEX outbound_attempts_reconcile_idx
ON outbound_attempts(state, updated_at_ms, id)
WHERE state IN ('accepted_pending', 'unknown_locked');

