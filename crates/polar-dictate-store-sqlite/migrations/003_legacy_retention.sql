ALTER TABLE dictate_recording ADD COLUMN legacy_pending INTEGER NOT NULL DEFAULT 0;
UPDATE dictate_recording SET legacy_pending=1,protected=1;
UPDATE dictate_legacy_import SET payload='' WHERE conflict=0;
