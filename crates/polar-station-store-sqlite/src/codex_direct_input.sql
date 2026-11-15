-- Add ordinary immediate input without reclassifying any existing queue receipt.
-- Rebuild only this CHECK-constrained table, preserving every column and index.
CREATE TABLE station_codex_submission_direct_input (
    intent_id TEXT PRIMARY KEY NOT NULL,
    principal TEXT NOT NULL,
    binding_id TEXT REFERENCES station_codex_binding(id),
    binding_revision INTEGER,
    operation_kind TEXT NOT NULL CHECK(operation_kind IN ('start','queue','steer','interrupt','reject')),
    expected_turn_id TEXT,
    source_kind TEXT NOT NULL CHECK(source_kind IN ('local','peer','telegram')),
    bot_id INTEGER,
    chat_id INTEGER,
    message_id INTEGER,
    update_id INTEGER,
    client_message_id TEXT,
    status TEXT NOT NULL CHECK(status IN ('received','submitting','accepted','rejected','unknown')),
    queue_id TEXT,
    turn_id TEXT,
    input_text TEXT CHECK(input_text IS NULL OR length(CAST(input_text AS BLOB)) <= 16384),
    input_digest TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    error_code TEXT,
    ack_status TEXT NOT NULL DEFAULT 'suppressed'
        CHECK (ack_status IN ('suppressed','pending','sending','sent','rejected','unknown','expired')),
    ack_message_id INTEGER,
    ack_error_code TEXT
);

INSERT INTO station_codex_submission_direct_input (intent_id, principal, binding_id, binding_revision, operation_kind, expected_turn_id, source_kind, bot_id, chat_id, message_id, update_id, client_message_id, status, queue_id, turn_id, input_text, input_digest, created_at_ms, updated_at_ms, expires_at_ms, error_code, ack_status, ack_message_id, ack_error_code)
SELECT intent_id, principal, binding_id, binding_revision, operation_kind, expected_turn_id, source_kind, bot_id, chat_id, message_id, update_id, client_message_id, status, queue_id, turn_id, input_text, input_digest, created_at_ms, updated_at_ms, expires_at_ms, error_code, ack_status, ack_message_id, ack_error_code FROM station_codex_submission;
DROP TABLE station_codex_submission;
ALTER TABLE station_codex_submission_direct_input RENAME TO station_codex_submission;

CREATE UNIQUE INDEX station_codex_client_message ON station_codex_submission(client_message_id) WHERE client_message_id IS NOT NULL;
CREATE UNIQUE INDEX station_codex_message ON station_codex_submission(bot_id,chat_id,message_id) WHERE message_id IS NOT NULL;
CREATE INDEX station_codex_submission_resolved_age
    ON station_codex_submission(updated_at_ms)
    WHERE status IN ('accepted','rejected') AND ack_status NOT IN ('pending','sending','unknown');
CREATE UNIQUE INDEX station_codex_update ON station_codex_submission(bot_id,update_id) WHERE bot_id IS NOT NULL AND update_id IS NOT NULL;
CREATE INDEX station_telegram_ack_pending ON station_codex_submission(bot_id,created_at_ms)
    WHERE source_kind='telegram' AND ack_status='pending';
