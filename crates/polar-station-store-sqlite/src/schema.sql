CREATE TABLE station_document (
    key TEXT PRIMARY KEY NOT NULL,
    payload BLOB NOT NULL
);
CREATE TABLE station_inbox_message (
    id TEXT PRIMARY KEY NOT NULL,
    created_at_ms INTEGER NOT NULL,
    payload_json TEXT NOT NULL
);
CREATE INDEX station_inbox_created_at ON station_inbox_message(created_at_ms DESC, id DESC);
