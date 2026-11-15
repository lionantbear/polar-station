-- Keep terminal JSON and its reply/approval workflow intact.
ALTER TABLE station_inbox_message ADD COLUMN record_kind TEXT NOT NULL DEFAULT 'terminal'
    CHECK (record_kind IN ('terminal','codex_completion'));
CREATE INDEX station_inbox_kind_created ON station_inbox_message(record_kind,created_at_ms DESC,id DESC);
CREATE INDEX station_codex_notification_completion ON station_codex_notification(binding_id,binding_revision,turn_id);
