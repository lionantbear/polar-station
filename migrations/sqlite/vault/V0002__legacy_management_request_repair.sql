-- The idempotent column repair is executed by the adapter's versioned
-- bootstrap callback in the same transaction as this migration ledger entry.
SELECT 1;
