-- Preserve v2 migration checksums and every delivery fact. Conflicting old
-- completion identities fail this transaction; no record is silently dropped.
ALTER TABLE station_codex_binding DROP COLUMN loaded_instance_id;
ALTER TABLE station_codex_checkpoint DROP COLUMN loaded_instance_id;
ALTER TABLE station_codex_submission DROP COLUMN loaded_instance_id;
CREATE TABLE station_codex_notification_v3 (
    id TEXT PRIMARY KEY NOT NULL,
    binding_id TEXT NOT NULL REFERENCES station_codex_binding(id),
    binding_revision INTEGER NOT NULL,
    turn_id TEXT NOT NULL,
    channel TEXT NOT NULL CHECK(channel IN ('telegram','imessage')),
    destination_key TEXT NOT NULL,
    bot_id INTEGER,
    chat_id INTEGER,
    sender_host_id TEXT,
    sender_generation TEXT,
    status TEXT NOT NULL CHECK(status IN ('pending','sending','sent','awaiting_receipt','failed','unknown','expired')),
    provider_message_id INTEGER,
    body_digest TEXT NOT NULL,
    bounded_body TEXT,
    created_at_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    error_code TEXT,
    UNIQUE(binding_id,binding_revision,turn_id,channel,destination_key)
);
INSERT INTO station_codex_notification_v3 (id,binding_id,binding_revision,turn_id,channel,destination_key,bot_id,chat_id,sender_host_id,sender_generation,status,provider_message_id,body_digest,bounded_body,created_at_ms,expires_at_ms,error_code) SELECT id,binding_id,binding_revision,turn_id,channel,destination_key,bot_id,chat_id,sender_host_id,sender_generation,status,provider_message_id,body_digest,bounded_body,created_at_ms,expires_at_ms,error_code FROM station_codex_notification;
DROP TABLE station_codex_notification;
ALTER TABLE station_codex_notification_v3 RENAME TO station_codex_notification;
CREATE UNIQUE INDEX station_codex_telegram_notification ON station_codex_notification(bot_id,chat_id,provider_message_id) WHERE provider_message_id IS NOT NULL;
