ALTER TABLE dictate_legacy_import ADD COLUMN deleted INTEGER NOT NULL DEFAULT 0;
CREATE INDEX dictate_legacy_deleted ON dictate_legacy_import(owner,id,deleted);
UPDATE dictate_legacy_import AS i SET deleted=1,payload='',conflict=0
WHERE NOT EXISTS (SELECT 1 FROM dictate_recording r WHERE r.owner=i.owner AND r.id=i.id)
AND NOT EXISTS (SELECT 1 FROM dictate_text_edit t WHERE t.owner=i.owner AND t.id=i.id);
