-- Only resolved history expires. Uncertain writes retain their identities.
CREATE INDEX station_codex_submission_resolved_age
    ON station_codex_submission(updated_at_ms)
    WHERE status IN ('accepted','rejected') AND ack_status NOT IN ('pending','sending','unknown');
CREATE INDEX station_codex_notification_resolved_age
    ON station_codex_notification(created_at_ms)
    WHERE status IN ('sent','failed','expired');
CREATE INDEX station_imessage_receipt_resolved_age
    ON station_imessage_receipt(updated_at_ms)
    WHERE status IN ('sent_to_messages','failed');
