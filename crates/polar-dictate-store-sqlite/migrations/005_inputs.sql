CREATE TABLE dictate_input_blob (
 owner TEXT NOT NULL, operation_id TEXT NOT NULL, digest TEXT NOT NULL, content TEXT NOT NULL,
 PRIMARY KEY(owner,operation_id,digest)
);
CREATE TABLE dictate_input (
 owner TEXT NOT NULL, operation_id TEXT NOT NULL, key TEXT NOT NULL,
 request_id TEXT NOT NULL, stage TEXT NOT NULL, channel TEXT NOT NULL,
 status TEXT NOT NULL, created_ms INTEGER NOT NULL, digest TEXT NOT NULL,
 PRIMARY KEY(owner,operation_id,key)
);
CREATE TRIGGER dictate_recording_delete_inputs AFTER DELETE ON dictate_recording BEGIN
 DELETE FROM dictate_input WHERE owner=OLD.owner AND operation_id=OLD.id;
 DELETE FROM dictate_input_blob WHERE owner=OLD.owner AND operation_id=OLD.id;
END;
CREATE TRIGGER dictate_text_delete_inputs AFTER DELETE ON dictate_text_edit BEGIN
 DELETE FROM dictate_input WHERE owner=OLD.owner AND operation_id=OLD.id;
 DELETE FROM dictate_input_blob WHERE owner=OLD.owner AND operation_id=OLD.id;
END;
