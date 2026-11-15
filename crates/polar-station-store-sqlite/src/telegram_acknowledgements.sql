-- Preserve older rows without sending acknowledgements retroactively.
ALTER TABLE station_codex_submission ADD COLUMN ack_status TEXT NOT NULL DEFAULT 'suppressed'
    CHECK (ack_status IN ('suppressed','pending','sending','sent','rejected','unknown','expired'));
ALTER TABLE station_codex_submission ADD COLUMN ack_message_id INTEGER;
ALTER TABLE station_codex_submission ADD COLUMN ack_error_code TEXT;
CREATE INDEX station_telegram_ack_pending ON station_codex_submission(bot_id,created_at_ms)
    WHERE source_kind='telegram' AND ack_status='pending';
