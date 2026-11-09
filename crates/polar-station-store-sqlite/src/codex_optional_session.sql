-- session_key is JSON; null denotes an observed Codex thread without a tmux association.
-- Preserve existing binding IDs, revisions, foreign keys and in-flight receipts.
DROP INDEX station_codex_active_session;
CREATE UNIQUE INDEX station_codex_active_session ON station_codex_binding(session_key)
    WHERE enabled=1 AND session_key <> 'null';
