CREATE TABLE attachment_transfer_cleanup (
    operation_id TEXT PRIMARY KEY NOT NULL,
    temporary_path TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0)
);

CREATE TABLE search_index_state (
    singleton INTEGER PRIMARY KEY NOT NULL CHECK (singleton = 1),
    document_version INTEGER NOT NULL CHECK (document_version >= 0)
);

INSERT INTO search_index_state(singleton, document_version) VALUES (1, 0);
