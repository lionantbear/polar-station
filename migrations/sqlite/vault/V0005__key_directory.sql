ALTER TABLE vault_secret ADD COLUMN device_only INTEGER NOT NULL DEFAULT 0 CHECK(device_only IN (0,1));
CREATE TABLE vault_key_directory (
 owner_node_id TEXT PRIMARY KEY,
 revision INTEGER NOT NULL CHECK(revision > 0),
 nonce BLOB NOT NULL CHECK(length(nonce) = 12),
 ciphertext BLOB NOT NULL CHECK(length(ciphertext) BETWEEN 16 AND 65536)
);
