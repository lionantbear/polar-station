CREATE TABLE dictate_recording (
 owner TEXT NOT NULL, id TEXT NOT NULL, created_ms INTEGER NOT NULL,
 capturing INTEGER NOT NULL, payload TEXT NOT NULL,
 PRIMARY KEY(owner,id)
);
CREATE UNIQUE INDEX dictate_one_capture ON dictate_recording(owner) WHERE capturing=1;
CREATE INDEX dictate_recent ON dictate_recording(owner,created_ms DESC,id);
CREATE TABLE dictate_audio_block (
 owner TEXT NOT NULL, recording_id TEXT NOT NULL, sequence INTEGER NOT NULL,
 start_sample INTEGER NOT NULL, pcm BLOB NOT NULL, digest BLOB NOT NULL,
 PRIMARY KEY(owner,recording_id,sequence),
 FOREIGN KEY(owner,recording_id) REFERENCES dictate_recording(owner,id) ON DELETE CASCADE
);
