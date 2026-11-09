CREATE TABLE vault_secret (
 secret_id TEXT PRIMARY KEY,
 owner_node_id TEXT NOT NULL,
 kind INTEGER NOT NULL CHECK(kind IN (1,2,3)),
 current_version INTEGER NOT NULL CHECK(current_version > 0),
 revision INTEGER NOT NULL CHECK(revision > 0),
 revoked INTEGER NOT NULL CHECK(revoked IN (0,1))
);
CREATE INDEX vault_secret_owner ON vault_secret(owner_node_id,secret_id);
CREATE TABLE vault_secret_version (
 secret_id TEXT NOT NULL REFERENCES vault_secret(secret_id),
 version INTEGER NOT NULL CHECK(version > 0),
 vault_id TEXT NOT NULL,
 key_generation INTEGER NOT NULL CHECK(key_generation = 1),
 nonce BLOB NOT NULL CHECK(length(nonce) = 12),
 ciphertext BLOB NOT NULL CHECK(length(ciphertext) BETWEEN 17 AND 65552),
 PRIMARY KEY(secret_id,version)
);
CREATE TABLE vault_secret_import (
 owner_node_id TEXT NOT NULL,
 import_id TEXT NOT NULL,
 secret_id TEXT NOT NULL REFERENCES vault_secret(secret_id),
 PRIMARY KEY(owner_node_id,import_id)
);
