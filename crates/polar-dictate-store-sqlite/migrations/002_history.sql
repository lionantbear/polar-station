ALTER TABLE dictate_recording ADD COLUMN summary TEXT;
ALTER TABLE dictate_recording ADD COLUMN visible INTEGER NOT NULL DEFAULT 0;
CREATE INDEX dictate_visible_recent ON dictate_recording(owner,visible,created_ms DESC,id DESC);
ALTER TABLE dictate_recording ADD COLUMN work_pending INTEGER NOT NULL DEFAULT 0;
ALTER TABLE dictate_recording ADD COLUMN recoverable INTEGER NOT NULL DEFAULT 0;
ALTER TABLE dictate_recording ADD COLUMN protected INTEGER NOT NULL DEFAULT 1;
ALTER TABLE dictate_recording ADD COLUMN completed_ms INTEGER;
ALTER TABLE dictate_recording ADD COLUMN audio_bytes INTEGER NOT NULL DEFAULT 0;
CREATE INDEX dictate_work ON dictate_recording(owner,work_pending,created_ms,id);
CREATE INDEX dictate_recovery ON dictate_recording(owner,recoverable,id);
CREATE INDEX dictate_retention ON dictate_recording(owner,protected,completed_ms,id);
CREATE TABLE dictate_text_edit (
 owner TEXT NOT NULL,id TEXT NOT NULL,created_ms INTEGER NOT NULL,
 payload TEXT NOT NULL,summary TEXT NOT NULL,protected INTEGER NOT NULL,
 completed_ms INTEGER,running INTEGER NOT NULL,
 PRIMARY KEY(owner,id)
);
CREATE INDEX dictate_text_recent ON dictate_text_edit(owner,created_ms DESC,id);
CREATE INDEX dictate_text_retention ON dictate_text_edit(owner,protected,completed_ms,id);
CREATE INDEX dictate_text_running ON dictate_text_edit(owner,running,id);
CREATE TABLE dictate_legacy_import (
 owner TEXT NOT NULL,id TEXT NOT NULL,digest TEXT NOT NULL,
 conflict INTEGER NOT NULL,payload TEXT NOT NULL,
 PRIMARY KEY(owner,id,digest)
);
