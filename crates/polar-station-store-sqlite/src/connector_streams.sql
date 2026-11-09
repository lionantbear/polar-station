-- Provider stream/receipt facts are distinct from the existing source outbox.
CREATE TABLE station_connector_stream (
    id TEXT PRIMARY KEY NOT NULL,
    connector_id TEXT NOT NULL,
    destination_ref TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    title TEXT NOT NULL,
    return_host_id TEXT NOT NULL,
    return_endpoint_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('pending','creating','ready','failed','unknown')),
    platform_id TEXT,
    error_code TEXT,
    created_at_ms INTEGER NOT NULL,
    provider_scope TEXT NOT NULL,
    UNIQUE(connector_id,destination_ref,thread_id)
);
CREATE UNIQUE INDEX station_connector_platform_stream
    ON station_connector_stream(connector_id,destination_ref,platform_id) WHERE platform_id IS NOT NULL;
CREATE TABLE station_connector_delivery (
    id TEXT PRIMARY KEY NOT NULL,
    stream_id TEXT NOT NULL REFERENCES station_connector_stream(id),
    turn_id TEXT NOT NULL,
    event_kind TEXT NOT NULL,
    content_digest TEXT NOT NULL,
    body TEXT,
    status TEXT NOT NULL CHECK(status IN ('pending','sending','sent','failed','unknown','expired')),
    provider_message_id TEXT,
    error_code TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    UNIQUE(stream_id,turn_id,event_kind)
);
CREATE TABLE station_connector_producer (
    delivery_id TEXT NOT NULL REFERENCES station_connector_delivery(id) ON DELETE CASCADE,
    source_host_id TEXT NOT NULL,
    PRIMARY KEY(delivery_id,source_host_id)
);
CREATE TABLE station_connector_inbound (
    id TEXT PRIMARY KEY NOT NULL,
    stream_id TEXT NOT NULL REFERENCES station_connector_stream(id),
    provider_message_id TEXT NOT NULL,
    return_host_id TEXT NOT NULL,
    body TEXT,
    content_digest TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('received','accepted','rejected','unknown','expired')),
    created_at_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    UNIQUE(stream_id,provider_message_id)
);
