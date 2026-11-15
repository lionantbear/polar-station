CREATE TABLE station_codex_binding (
    id TEXT PRIMARY KEY NOT NULL,
    owner_host_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK(revision > 0),
    session_key TEXT NOT NULL,
    endpoint_id TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    loaded_instance_id TEXT NOT NULL,
    enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),
    telegram_destination_ref TEXT,
    imessage_enabled INTEGER NOT NULL DEFAULT 0 CHECK(imessage_enabled IN (0,1)),
    bound_at_ms INTEGER NOT NULL
);
CREATE UNIQUE INDEX station_codex_active_session ON station_codex_binding(session_key) WHERE enabled=1;
CREATE UNIQUE INDEX station_codex_active_thread ON station_codex_binding(endpoint_id,thread_id) WHERE enabled=1;
CREATE TABLE station_codex_checkpoint (
    binding_id TEXT PRIMARY KEY NOT NULL REFERENCES station_codex_binding(id),
    binding_revision INTEGER NOT NULL,
    loaded_instance_id TEXT NOT NULL,
    last_reconciled_turn TEXT,
    bound_at_ms INTEGER NOT NULL,
    reconcile_status TEXT NOT NULL
);
CREATE TABLE station_codex_notification (
    id TEXT PRIMARY KEY NOT NULL,
    binding_id TEXT NOT NULL REFERENCES station_codex_binding(id),
    binding_revision INTEGER NOT NULL,
    loaded_instance_id TEXT NOT NULL,
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
    UNIQUE(binding_id,binding_revision,loaded_instance_id,turn_id,channel,destination_key)
);
CREATE UNIQUE INDEX station_codex_telegram_notification ON station_codex_notification(bot_id,chat_id,provider_message_id) WHERE provider_message_id IS NOT NULL;
CREATE TABLE station_codex_submission (
    intent_id TEXT PRIMARY KEY NOT NULL,
    principal TEXT NOT NULL,
    binding_id TEXT REFERENCES station_codex_binding(id),
    binding_revision INTEGER,
    loaded_instance_id TEXT,
    operation_kind TEXT NOT NULL CHECK(operation_kind IN ('queue','steer','interrupt','reject')),
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
    error_code TEXT
);
CREATE UNIQUE INDEX station_codex_update ON station_codex_submission(bot_id,update_id) WHERE bot_id IS NOT NULL AND update_id IS NOT NULL;
CREATE UNIQUE INDEX station_codex_message ON station_codex_submission(bot_id,chat_id,message_id) WHERE message_id IS NOT NULL;
CREATE UNIQUE INDEX station_codex_client_message ON station_codex_submission(client_message_id) WHERE client_message_id IS NOT NULL;
CREATE TABLE station_telegram_cursor (
    bot_id INTEGER PRIMARY KEY NOT NULL,
    next_offset INTEGER NOT NULL CHECK(next_offset >= 0),
    updated_at_ms INTEGER NOT NULL
);
CREATE TABLE station_imessage_receipt (
    source_principal TEXT NOT NULL,
    request_id TEXT NOT NULL,
    sender_generation TEXT NOT NULL,
    destination_ref TEXT NOT NULL,
    content_digest TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('sending','sent_to_messages','failed','unknown')),
    error_code TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY(source_principal,request_id)
);
