ALTER TABLE vault_secret ADD COLUMN name TEXT NOT NULL DEFAULT '';
UPDATE vault_secret SET name=secret_id;
CREATE INDEX vault_secret_owner_name ON vault_secret(owner_node_id,name,secret_id);
