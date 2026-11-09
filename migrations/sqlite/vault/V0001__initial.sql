CREATE TABLE IF NOT EXISTS vault_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL,updated_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS node_settings(setting_key TEXT PRIMARY KEY,setting_value TEXT NOT NULL,updated_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS admin_secrets(secret_name TEXT PRIMARY KEY,nonce BLOB NOT NULL,ciphertext BLOB NOT NULL,updated_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS trust_nodes(
 peer_node_id TEXT PRIMARY KEY,peer_node_name TEXT NOT NULL,binding_pubkey TEXT NOT NULL,
 outbound_state TEXT NOT NULL CHECK(outbound_state IN ('none','pending','trusted','revoked')),
 inbound_state TEXT NOT NULL CHECK(inbound_state IN ('none','pending','trusted','revoked')),
 identity_change_pending INTEGER NOT NULL CHECK(identity_change_pending IN (0,1)),
 approved_at INTEGER,revoked_at INTEGER,created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_trust_nodes_name ON trust_nodes(peer_node_name);
CREATE TABLE IF NOT EXISTS capability_security_state(
 name TEXT PRIMARY KEY,status TEXT NOT NULL,version TEXT,installed_at INTEGER,last_healthcheck_at INTEGER,
 auth_present INTEGER NOT NULL CHECK(auth_present IN (0,1)),auth_sha256 TEXT,created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS secret_material(
 secret_scope TEXT NOT NULL,secret_name TEXT NOT NULL,nonce BLOB NOT NULL,ciphertext BLOB NOT NULL,
 created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL,PRIMARY KEY(secret_scope,secret_name)
);
CREATE TABLE IF NOT EXISTS runtime_dedup(dedup_key TEXT PRIMARY KEY,observed_value TEXT NOT NULL,expires_at INTEGER,created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS control_authority(
 peer_node_id TEXT PRIMARY KEY,granted_at INTEGER NOT NULL,granted_by TEXT NOT NULL,
 scope TEXT NOT NULL CHECK(scope IN ('read_only','full')),created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS managed_nodes(peer_node_id TEXT PRIMARY KEY,recorded_at INTEGER NOT NULL,recorded_by TEXT NOT NULL,created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS management_requests(
 request_id TEXT PRIMARY KEY,requester_node_id TEXT NOT NULL,requester_node_name TEXT,manager_node_id TEXT NOT NULL,
 channel_id TEXT NOT NULL,thread_id TEXT NOT NULL,request_message_id TEXT NOT NULL,response_message_id TEXT,
 request_transport_plane TEXT NOT NULL,target_channel_id TEXT,target_thread_id TEXT,target_inhouse_polar_id TEXT,
 requested_scope TEXT NOT NULL CHECK(requested_scope IN ('read_only','full')),request_text TEXT,endpoint_hint TEXT,
 status TEXT NOT NULL CHECK(status IN ('pending','approved','rejected')),requested_at INTEGER NOT NULL,resolved_at INTEGER,
 created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_management_requests_peer_status ON management_requests(requester_node_id,status,requested_at DESC);
DROP TABLE IF EXISTS approved_management_peer_routes;
