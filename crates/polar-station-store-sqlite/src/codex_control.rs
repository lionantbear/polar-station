//! Physical Codex/channel facts. Authorization and RPC decisions belong to Host.
use crate::{HostDatabase, StoreError};
use polar_station_inbox::{CodexCompletion, CodexCompletionCursor};
use rusqlite::{params, OptionalExtension, TransactionBehavior};

const COMPLETION_SELECT: &str =
    "SELECT payload_json FROM station_inbox_message WHERE record_kind='codex_completion'
                AND (created_at_ms<?1 OR (created_at_ms=?1 AND (?2 IS NULL OR id<?2)))
                AND json_extract(payload_json,'$.origin.owner_host_id')=?3
                AND (?4 IS NULL OR json_extract(payload_json,'$.origin.endpoint_id')=?4)
                ORDER BY created_at_ms DESC,id DESC LIMIT ?5";

/// A bounded completion and its delivery records from the same SQLite snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexCompletionWithNotifications {
    pub completion: CodexCompletion,
    pub notifications: Vec<CodexNotificationRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexBindingRow {
    pub id: String,
    pub owner_host_id: String,
    pub revision: i64,
    pub session_key: String,
    pub endpoint_id: String,
    pub thread_id: String,
    pub enabled: bool,
    pub telegram_destination_ref: Option<String>,
    pub imessage_enabled: bool,
    pub bound_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexSubmissionRow {
    pub intent_id: String,
    pub principal: String,
    pub binding_id: Option<String>,
    pub binding_revision: Option<i64>,
    pub operation_kind: String,
    pub expected_turn_id: Option<String>,
    pub source_kind: String,
    pub bot_id: Option<i64>,
    pub chat_id: Option<i64>,
    pub message_id: Option<i64>,
    pub update_id: Option<i64>,
    pub client_message_id: Option<String>,
    pub status: String,
    pub queue_id: Option<String>,
    pub turn_id: Option<String>,
    pub input_text: Option<String>,
    pub input_digest: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub expires_at_ms: i64,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmissionTransition {
    pub status: String,
    pub queue_id: Option<String>,
    pub turn_id: Option<String>,
    pub error_code: Option<String>,
    pub at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexNotificationRow {
    pub id: String,
    pub binding_id: String,
    pub binding_revision: i64,
    pub turn_id: String,
    pub channel: String,
    pub destination_key: String,
    pub bot_id: Option<i64>,
    pub chat_id: Option<i64>,
    pub sender_host_id: Option<String>,
    pub sender_generation: Option<String>,
    pub status: String,
    pub provider_message_id: Option<i64>,
    pub body_digest: String,
    pub bounded_body: Option<String>,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexCheckpointRow {
    pub binding_id: String,
    pub binding_revision: i64,
    pub last_reconciled_turn: Option<String>,
    pub bound_at_ms: i64,
    pub reconcile_status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImessageReceiptRow {
    pub source_principal: String,
    pub request_id: String,
    pub sender_generation: String,
    pub destination_ref: String,
    pub content_digest: String,
    pub status: String,
    pub error_code: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone)]
pub struct SqliteCodexStore {
    pub(super) database: HostDatabase,
}

/// Join an existing settings transaction; never leave revisions partly advanced.
/// False means one active binding exhausted the SQLite integer revision range.
pub fn advance_codex_binding_revisions(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<bool> {
    let exhausted: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM station_codex_binding WHERE enabled=1 AND revision=9223372036854775807)", [], |r| r.get(0))?;
    if exhausted {
        return Ok(false);
    }
    tx.execute(
        "UPDATE station_codex_binding SET revision=revision+1 WHERE enabled=1",
        [],
    )?;
    Ok(true)
}

impl SqliteCodexStore {
    pub fn from_database(database: HostDatabase) -> Result<Self, StoreError> {
        database.runtime()?;
        Ok(Self { database })
    }

    /// Uses a revision condition, including revision zero for first insertion.
    pub fn save_binding(
        &self,
        expected_revision: i64,
        row: CodexBindingRow,
    ) -> Result<bool, StoreError> {
        Ok(self.database.runtime()?.write_blocking(move |db| {
            db.execute("INSERT INTO station_codex_binding(id,owner_host_id,revision,session_key,endpoint_id,thread_id,enabled,telegram_destination_ref,imessage_enabled,bound_at_ms)
                SELECT ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10 WHERE ?3=?11+1 AND (?11=0 OR EXISTS(SELECT 1 FROM station_codex_binding WHERE id=?1 AND revision=?11))
                ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,session_key=excluded.session_key,endpoint_id=excluded.endpoint_id,thread_id=excluded.thread_id,enabled=excluded.enabled,telegram_destination_ref=excluded.telegram_destination_ref,imessage_enabled=excluded.imessage_enabled,bound_at_ms=excluded.bound_at_ms
                WHERE station_codex_binding.revision=?11 AND station_codex_binding.owner_host_id=excluded.owner_host_id",
                params![row.id,row.owner_host_id,row.revision,row.session_key,row.endpoint_id,row.thread_id,row.enabled,row.telegram_destination_ref,row.imessage_enabled,row.bound_at_ms,expected_revision])
        })? == 1)
    }

    /// Terminal presentation association cannot revoke original-thread channel receipts.
    pub fn associate_session(
        &self,
        id: String,
        expected_revision: i64,
        session_key: String,
    ) -> Result<bool, StoreError> {
        self.database.runtime()?.write_blocking(move |db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM station_codex_binding WHERE id=?1 AND revision=?2 AND enabled=1)", params![id, expected_revision], |r| r.get(0))?;
            if !current { return Ok(false); }
            if session_key != "null" {
                tx.execute("UPDATE station_codex_binding SET session_key='null' WHERE session_key=?1 AND enabled=1", [&session_key])?;
            }
            tx.execute("UPDATE station_codex_binding SET session_key=?1 WHERE id=?2", params![session_key,id])?;
            tx.commit()?;
            Ok(true)
        }).map_err(Into::into)
    }

    /// Exact primary-key lookup; policy checks remain with the Host.
    pub fn binding(&self, id: &str) -> Result<Option<CodexBindingRow>, StoreError> {
        let id = id.to_owned();
        Ok(self.database.runtime()?.read_blocking(move |db| {
            db.prepare_cached("SELECT id,owner_host_id,revision,session_key,endpoint_id,thread_id,enabled,telegram_destination_ref,imessage_enabled,bound_at_ms FROM station_codex_binding WHERE id=?1")?
                .query_row([id], decode_binding).optional()
        })?)
    }

    pub fn bindings(&self) -> Result<Vec<CodexBindingRow>, StoreError> {
        Ok(self.database.runtime()?.read_blocking(|db| {
            let mut q=db.prepare("SELECT id,owner_host_id,revision,session_key,endpoint_id,thread_id,enabled,telegram_destination_ref,imessage_enabled,bound_at_ms FROM station_codex_binding ORDER BY id")?;
            let rows = q.query_map([], decode_binding)?;
            rows.collect()
        })?)
    }

    pub fn insert_submission(&self, row: CodexSubmissionRow) -> Result<bool, StoreError> {
        Ok(self
            .database
            .runtime()?
            .write_blocking(move |db| insert_submission(db, &row))?
            == 1)
    }

    /// Rejected and accepted update facts commit together with the provider cursor.
    pub fn accept_telegram_batch(
        &self,
        bot_id: i64,
        next_offset: i64,
        now_ms: i64,
        rows: Vec<CodexSubmissionRow>,
    ) -> Result<(), StoreError> {
        self.database.runtime()?.write_blocking(move |db| {
            let tx=db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            for row in rows { insert_submission(&tx, &row)?; }
            tx.execute("INSERT INTO station_telegram_cursor(bot_id,next_offset,updated_at_ms) VALUES (?1,?2,?3)
                ON CONFLICT(bot_id) DO UPDATE SET next_offset=max(next_offset,excluded.next_offset),updated_at_ms=excluded.updated_at_ms", params![bot_id,next_offset,now_ms])?;
            tx.commit()
        })?;
        Ok(())
    }

    pub fn telegram_cursor(&self, bot_id: i64) -> Result<Option<i64>, StoreError> {
        Ok(self.database.runtime()?.read_blocking(move |db| {
            db.query_row(
                "SELECT next_offset FROM station_telegram_cursor WHERE bot_id=?1",
                [bot_id],
                |row| row.get(0),
            )
            .optional()
        })?)
    }

    pub fn submission(&self, intent_id: &str) -> Result<Option<CodexSubmissionRow>, StoreError> {
        let id = intent_id.to_owned();
        Ok(self.database.runtime()?.read_blocking(move |db| {
            db.query_row(
                &format!("{SUBMISSION_SELECT} WHERE intent_id=?1"),
                [id],
                submission_from_row,
            )
            .optional()
        })?)
    }

    /// Only the expected state can advance; an accepted result never becomes rejected.
    pub fn transition_submission(
        &self,
        id: &str,
        expected: &str,
        next: SubmissionTransition,
    ) -> Result<bool, StoreError> {
        let (id, expected) = (id.to_owned(), expected.to_owned());
        Ok(self.database.runtime()?.write_blocking(move |db| db.execute("UPDATE station_codex_submission SET status=?3,queue_id=?4,turn_id=?5,error_code=?6,updated_at_ms=?7,input_text=CASE WHEN ?3='submitting' THEN input_text ELSE NULL END WHERE intent_id=?1 AND status=?2 AND ((status='received' AND ?3 IN ('submitting','rejected')) OR (status='submitting' AND ?3 IN ('accepted','rejected','unknown')) OR (status='unknown' AND ?3='accepted'))",
            params![id,expected,next.status,next.queue_id,next.turn_id,next.error_code,next.at_ms]))? == 1)
    }

    pub fn received_telegram_submissions(
        &self,
        bot_id: i64,
    ) -> Result<Vec<CodexSubmissionRow>, StoreError> {
        Ok(self.database.runtime()?.read_blocking(move |db| {
            let mut query = db.prepare(&format!("{SUBMISSION_SELECT} WHERE bot_id=?1 AND source_kind='telegram' AND status='received' ORDER BY created_at_ms,intent_id LIMIT 100"))?;
            let rows = query.query_map([bot_id], submission_from_row)?;
            rows.collect()
        })?)
    }

    pub fn pending_telegram_acknowledgements(
        &self,
        bot_id: i64,
        now: i64,
    ) -> Result<Vec<CodexSubmissionRow>, StoreError> {
        Ok(self.database.runtime()?.read_blocking(move |db| {
            let mut query = db.prepare(&format!("{SUBMISSION_SELECT} WHERE bot_id=?1 AND source_kind='telegram' AND ack_status='pending' AND status IN ('accepted','rejected','unknown') AND expires_at_ms>?2 ORDER BY created_at_ms,intent_id LIMIT 100"))?;
            let rows = query.query_map(params![bot_id,now], submission_from_row)?;
            rows.collect()
        })?)
    }

    pub fn transition_telegram_ack(
        &self,
        id: &str,
        expected: &str,
        status: &str,
        message_id: Option<i64>,
        error: Option<String>,
    ) -> Result<bool, StoreError> {
        let (id, expected, status) = (id.to_owned(), expected.to_owned(), status.to_owned());
        Ok(self.database.runtime()?.write_blocking(move |db| {
            db.execute("UPDATE station_codex_submission SET ack_status=?3,ack_message_id=coalesce(?4,ack_message_id),ack_error_code=?5 WHERE intent_id=?1 AND source_kind='telegram' AND ack_status=?2 AND ((ack_status='pending' AND ?3 IN ('sending','suppressed','expired')) OR (ack_status='sending' AND ?3 IN ('pending','sent','rejected','unknown')))", params![id,expected,status,message_id,error])
        })? == 1)
    }

    pub fn pending_telegram_notifications(
        &self,
        bot_id: i64,
        now: i64,
    ) -> Result<Vec<CodexNotificationRow>, StoreError> {
        Ok(self.database.runtime()?.read_blocking(move |db| {
            let mut query = db.prepare(&format!("{NOTIFICATION_SELECT} WHERE channel='telegram' AND bot_id=?1 AND status='pending' AND expires_at_ms>?2 ORDER BY created_at_ms,id LIMIT 64"))?;
            let rows = query.query_map(params![bot_id,now], notification_from_row)?;
            rows.collect()
        })?)
    }

    /// Cancellation has the same uncertainty as a lost reply, confined to this bot.
    pub fn interrupt_telegram_io(&self, bot_id: i64, now: i64) -> Result<(), StoreError> {
        self.database.runtime()?.write_blocking(move |db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute("UPDATE station_codex_submission SET status='unknown',input_text=NULL,error_code='channel_interrupted',updated_at_ms=?2 WHERE bot_id=?1 AND source_kind='telegram' AND status='submitting'",params![bot_id,now])?;
            tx.execute("UPDATE station_codex_submission SET ack_status='unknown',ack_error_code='channel_interrupted' WHERE bot_id=?1 AND source_kind='telegram' AND ack_status='sending'",[bot_id])?;
            tx.execute("UPDATE station_codex_notification SET status='unknown',bounded_body=NULL,error_code='channel_interrupted' WHERE bot_id=?1 AND channel='telegram' AND status='sending'",[bot_id])?;
            tx.commit()
        })?;
        Ok(())
    }

    pub fn checkpoint(&self, binding_id: &str) -> Result<Option<CodexCheckpointRow>, StoreError> {
        let id = binding_id.to_owned();
        Ok(self.database.runtime()?.read_blocking(move |db| db.query_row("SELECT binding_id,binding_revision,last_reconciled_turn,bound_at_ms,reconcile_status FROM station_codex_checkpoint WHERE binding_id=?1",[id],|r| Ok(CodexCheckpointRow { binding_id:r.get(0)?,binding_revision:r.get(1)?,last_reconciled_turn:r.get(2)?,bound_at_ms:r.get(3)?,reconcile_status:r.get(4)? })).optional())?)
    }

    /// Completion facts and the reconcile watermark become durable together.
    pub fn record_completion(
        &self,
        checkpoint: CodexCheckpointRow,
        notifications: Vec<CodexNotificationRow>,
    ) -> Result<(), StoreError> {
        self.record_completion_with_inbox(checkpoint, notifications, None)
    }

    pub fn record_completion_with_inbox(
        &self,
        checkpoint: CodexCheckpointRow,
        notifications: Vec<CodexNotificationRow>,
        completion: Option<CodexCompletion>,
    ) -> Result<(), StoreError> {
        let inbox = completion
            .map(|record| {
                record.validate()?;
                Ok::<_, StoreError>((
                    record.id.as_str().to_owned(),
                    record.created_at_ms as i64,
                    serde_json::to_string(&record)?,
                ))
            })
            .transpose()?;
        self.database.runtime()?.write_blocking(move |db| {
            let tx=db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some((id, created, payload)) = inbox {
                tx.execute("INSERT INTO station_inbox_message(id,created_at_ms,payload_json,record_kind) VALUES (?1,?2,?3,'codex_completion') ON CONFLICT(id) DO NOTHING", params![id, created, payload])?;
            }
            for row in notifications { insert_notification(&tx,&row)?; }
            tx.execute("INSERT INTO station_codex_checkpoint(binding_id,binding_revision,last_reconciled_turn,bound_at_ms,reconcile_status) VALUES (?1,?2,?3,?4,?5) ON CONFLICT(binding_id) DO UPDATE SET binding_revision=excluded.binding_revision,last_reconciled_turn=excluded.last_reconciled_turn,bound_at_ms=excluded.bound_at_ms,reconcile_status=excluded.reconcile_status",
                params![checkpoint.binding_id,checkpoint.binding_revision,checkpoint.last_reconciled_turn,checkpoint.bound_at_ms,checkpoint.reconcile_status])?;
            tx.commit()
        })?;
        Ok(())
    }

    pub fn completions(
        &self,
        before: Option<CodexCompletionCursor>,
        limit: usize,
        owner: &str,
        endpoint: Option<&str>,
    ) -> Result<Vec<CodexCompletion>, StoreError> {
        if let Some(cursor) = &before {
            cursor.validate()?;
        }
        let before_time = before.as_ref().map_or(i64::MAX, |c| c.created_at_ms as i64);
        let before_id = before.map(|c| c.id.as_str().to_owned());
        let owner = owner.to_owned();
        let endpoint = endpoint.map(str::to_owned);
        let limit = limit.clamp(1, 33) as i64;
        let payloads = self.database.runtime()?.read_blocking(move |db| {
            let mut q = db.prepare(COMPLETION_SELECT)?;
            let rows = q.query_map(
                params![before_time, before_id, owner, endpoint, limit],
                |r| r.get::<_, String>(0),
            )?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })?;
        payloads
            .into_iter()
            .map(|p| {
                let record: CodexCompletion = serde_json::from_str(&p)?;
                record.validate()?;
                Ok(record)
            })
            .collect()
    }

    /// Two SELECTs in one read transaction, independent of page length.
    pub fn completion_page_with_notifications(
        &self,
        before: Option<CodexCompletionCursor>,
        limit: usize,
        owner: &str,
        endpoint: Option<&str>,
    ) -> Result<Vec<CodexCompletionWithNotifications>, StoreError> {
        if let Some(cursor) = &before {
            cursor.validate()?;
        }
        let before_time = before.as_ref().map_or(i64::MAX, |c| c.created_at_ms as i64);
        let before_id = before.map(|c| c.id.as_str().to_owned());
        let owner = owner.to_owned();
        let endpoint = endpoint.map(str::to_owned);
        let limit = limit.clamp(1, 33) as i64;
        self.database.runtime()?.read_blocking(move |db| {
            // Preserve domain decode errors while keeping both SELECTs in one snapshot.
            Ok((|| -> Result<_, StoreError> {
                let tx = db.unchecked_transaction()?;
                let payloads = {
                    let mut q = tx.prepare_cached(COMPLETION_SELECT)?;
                    let rows = q.query_map(params![before_time, before_id, owner, endpoint, limit], |r| r.get::<_, String>(0))?;
                    rows.collect::<rusqlite::Result<Vec<_>>>()?
                };
                let records = payloads.into_iter().map(|payload| {
                    let record: CodexCompletion = serde_json::from_str(&payload)?;
                    record.validate()?;
                    Ok(record)
                }).collect::<Result<Vec<_>, StoreError>>()?;
                let keys = records.iter().map(|record| {
                    let origin = &record.origin;
                    let revision = i64::try_from(origin.binding_revision)
                        .map_err(|_| polar_station_inbox::InboxError::InvalidText)?;
                    Ok((&origin.binding_id, revision, &origin.turn_id))
                }).collect::<Result<Vec<_>, StoreError>>()?;
                let keys = serde_json::to_string(&keys)?;
                // Reuse selected identities instead of re-running the page query.
                // Tuple membership allows the existing binding index to be used.
                let select = NOTIFICATION_SELECT.replace("FROM station_codex_notification", "FROM ranked");
                let sql = format!("WITH ranked AS (
                    SELECT *, row_number() OVER (
                        PARTITION BY binding_id,binding_revision,turn_id ORDER BY channel,id
                    ) AS page_rank FROM station_codex_notification
                    WHERE (binding_id,binding_revision,turn_id) IN (
                        SELECT json_extract(value,'$[0]'),json_extract(value,'$[1]'),
                               json_extract(value,'$[2]') FROM json_each(?1)
                    )
                ) {select} WHERE page_rank<=64 ORDER BY binding_id,binding_revision,turn_id,channel,id");
                let notifications = {
                    let mut q = tx.prepare_cached(&sql)?;
                    let rows = q.query_map([keys], notification_from_row)?;
                    rows.collect::<rusqlite::Result<Vec<_>>>()?
                };
                tx.commit()?;
                let mut grouped = std::collections::BTreeMap::<_, Vec<_>>::new();
                for row in notifications {
                    grouped.entry((row.binding_id.clone(), row.binding_revision, row.turn_id.clone()))
                        .or_default().push(row);
                }
                Ok(records.into_iter().map(|completion| {
                    let origin = &completion.origin;
                    // The checked conversion above covers every selected record.
                    let notifications = grouped.get(&(origin.binding_id.clone(), origin.binding_revision as i64, origin.turn_id.clone()))
                        .cloned().unwrap_or_default();
                    CodexCompletionWithNotifications { completion, notifications }
                }).collect())
            })())
        })?
    }

    pub fn completion_notifications(
        &self,
        binding_id: &str,
        revision: u64,
        turn_id: &str,
    ) -> Result<Vec<CodexNotificationRow>, StoreError> {
        let revision =
            i64::try_from(revision).map_err(|_| polar_station_inbox::InboxError::InvalidText)?;
        let (binding, turn) = (binding_id.to_owned(), turn_id.to_owned());
        Ok(self.database.runtime()?.read_blocking(move |db| {
            let mut q = db.prepare(&format!("{NOTIFICATION_SELECT} WHERE binding_id=?1 AND binding_revision=?2 AND turn_id=?3 ORDER BY channel,id LIMIT 64"))?;
            let rows = q.query_map(params![binding,revision,turn], notification_from_row)?;
            rows.collect()
        })?)
    }

    pub fn notification(&self, id: &str) -> Result<Option<CodexNotificationRow>, StoreError> {
        let id = id.to_owned();
        Ok(self.database.runtime()?.read_blocking(move |db| {
            db.query_row(
                &format!("{NOTIFICATION_SELECT} WHERE id=?1"),
                [id],
                notification_from_row,
            )
            .optional()
        })?)
    }

    pub fn telegram_notification(
        &self,
        bot_id: i64,
        chat_id: i64,
        message_id: i64,
    ) -> Result<Option<CodexNotificationRow>, StoreError> {
        Ok(self.database.runtime()?.read_blocking(move |db| db.query_row(&format!("{NOTIFICATION_SELECT} WHERE bot_id=?1 AND chat_id=?2 AND provider_message_id=?3 AND channel='telegram' AND status='sent'"),params![bot_id,chat_id,message_id],notification_from_row).optional())?)
    }

    pub fn pending_notifications(
        &self,
        now_ms: i64,
    ) -> Result<Vec<CodexNotificationRow>, StoreError> {
        Ok(self.database.runtime()?.read_blocking(move |db| {
            let mut q=db.prepare(&format!("{NOTIFICATION_SELECT} WHERE status IN ('pending','awaiting_receipt') AND expires_at_ms>?1 ORDER BY created_at_ms,id LIMIT 64"))?;
            let rows=q.query_map([now_ms],notification_from_row)?;
            rows.collect()
        })?)
    }

    pub fn pending_connector_notifications(
        &self,
        now_ms: i64,
    ) -> Result<Vec<CodexNotificationRow>, StoreError> {
        Ok(self.database.runtime()?.read_blocking(move |db| {
            let mut q=db.prepare(&format!("{NOTIFICATION_SELECT} WHERE destination_key LIKE 'connector:%' AND status IN ('pending','awaiting_receipt') AND expires_at_ms>?1 ORDER BY created_at_ms,id LIMIT 64"))?;
            let rows=q.query_map([now_ms],notification_from_row)?;
            rows.collect()
        })?)
    }
    pub fn pending_imessage_notifications(
        &self,
        now_ms: i64,
    ) -> Result<Vec<CodexNotificationRow>, StoreError> {
        Ok(self.database.runtime()?.read_blocking(move |db| {
            let mut q=db.prepare(&format!("{NOTIFICATION_SELECT} WHERE channel='imessage' AND destination_key NOT LIKE 'connector:%' AND status IN ('pending','awaiting_receipt') AND expires_at_ms>?1 ORDER BY created_at_ms,id LIMIT 64"))?;
            let rows=q.query_map([now_ms],notification_from_row)?;
            rows.collect()
        })?)
    }

    pub fn interrupt_imessage_io(&self) -> Result<(), StoreError> {
        self.database.runtime()?.write_blocking(|db| db.execute("UPDATE station_codex_notification SET status='awaiting_receipt',error_code='channel_interrupted' WHERE channel='imessage' AND status='sending'", []))?;
        Ok(())
    }

    pub fn transition_notification(
        &self,
        id: &str,
        expected: &str,
        status: &str,
        message_id: Option<i64>,
        error_code: Option<String>,
    ) -> Result<bool, StoreError> {
        let (id, expected, status) = (id.to_owned(), expected.to_owned(), status.to_owned());
        Ok(self.database.runtime()?.write_blocking(move |db| db.execute("UPDATE station_codex_notification SET status=?3,provider_message_id=coalesce(?4,provider_message_id),error_code=?5,bounded_body=CASE WHEN ?3 IN ('pending','sending','awaiting_receipt') THEN bounded_body ELSE NULL END WHERE id=?1 AND status=?2 AND status IN ('pending','sending','awaiting_receipt')",params![id,expected,status,message_id,error_code]))?==1)
    }

    /// The primary key is also the platform-send claim. Call the platform only if true.
    pub fn claim_imessage(&self, row: ImessageReceiptRow) -> Result<bool, StoreError> {
        Ok(self.database.runtime()?.write_blocking(move |db| db.execute("INSERT INTO station_imessage_receipt(source_principal,request_id,sender_generation,destination_ref,content_digest,status,error_code,created_at_ms,updated_at_ms) VALUES (?1,?2,?3,?4,?5,'sending',NULL,?6,?6) ON CONFLICT(source_principal,request_id) DO NOTHING",params![row.source_principal,row.request_id,row.sender_generation,row.destination_ref,row.content_digest,row.created_at_ms]))?==1)
    }

    pub fn imessage_receipt(
        &self,
        principal: &str,
        request_id: &str,
    ) -> Result<Option<ImessageReceiptRow>, StoreError> {
        let (principal, id) = (principal.to_owned(), request_id.to_owned());
        Ok(self.database.runtime()?.read_blocking(move |db| db.query_row("SELECT source_principal,request_id,sender_generation,destination_ref,content_digest,status,error_code,created_at_ms,updated_at_ms FROM station_imessage_receipt WHERE source_principal=?1 AND request_id=?2",params![principal,id],|r| Ok(ImessageReceiptRow {source_principal:r.get(0)?,request_id:r.get(1)?,sender_generation:r.get(2)?,destination_ref:r.get(3)?,content_digest:r.get(4)?,status:r.get(5)?,error_code:r.get(6)?,created_at_ms:r.get(7)?,updated_at_ms:r.get(8)?})).optional())?)
    }

    pub fn finish_imessage(
        &self,
        principal: &str,
        request_id: &str,
        status: &str,
        error: Option<String>,
        now_ms: i64,
    ) -> Result<bool, StoreError> {
        let (principal, id, status) = (
            principal.to_owned(),
            request_id.to_owned(),
            status.to_owned(),
        );
        Ok(self.database.runtime()?.write_blocking(move |db| db.execute("UPDATE station_imessage_receipt SET status=?3,error_code=?4,updated_at_ms=?5 WHERE source_principal=?1 AND request_id=?2 AND status='sending'",params![principal,id,status,error,now_ms]))?==1)
    }

    /// Erase transient text and resolved history; retain uncertain write identities.
    pub fn expire_transient_text(&self, now_ms: i64) -> Result<(), StoreError> {
        self.database.runtime()?.write_blocking(move |db| {
            let tx=db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute("UPDATE station_codex_submission SET input_text=NULL,status=CASE WHEN status='received' THEN 'rejected' ELSE status END,error_code=CASE WHEN status='received' THEN 'expired' ELSE error_code END,updated_at_ms=?1 WHERE expires_at_ms<=?1 AND input_text IS NOT NULL",[now_ms])?;
            tx.execute("UPDATE station_codex_submission SET ack_status='expired' WHERE ack_status='pending' AND expires_at_ms<=?1", [now_ms])?;
            tx.execute("UPDATE station_codex_notification SET bounded_body=NULL,status=CASE WHEN status='pending' THEN 'expired' WHEN status='awaiting_receipt' THEN 'unknown' ELSE status END WHERE expires_at_ms<=?1 AND bounded_body IS NOT NULL",[now_ms])?;
            let cutoff = now_ms.saturating_sub(polar_station_inbox::RETENTION_MS as i64);
            tx.execute("DELETE FROM station_codex_submission WHERE updated_at_ms<?1 AND status IN ('accepted','rejected') AND ack_status NOT IN ('pending','sending','unknown')", [cutoff])?;
            tx.execute("DELETE FROM station_codex_notification WHERE created_at_ms<?1 AND status IN ('sent','failed','expired')", [cutoff])?;
            tx.execute("DELETE FROM station_imessage_receipt WHERE updated_at_ms<?1 AND status IN ('sent_to_messages','failed')", [cutoff])?;
            tx.execute("DELETE FROM station_inbox_message WHERE record_kind='codex_completion' AND created_at_ms<?1", [cutoff])?;
            tx.commit()
        })?;
        Ok(())
    }

    /// Recovery records uncertainty instead of replaying writes whose replies were lost.
    pub fn recover_after_restart(&self, now_ms: i64) -> Result<(), StoreError> {
        self.database.runtime()?.write_blocking(move |db| {
            let tx=db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute("UPDATE station_codex_submission SET status='unknown',input_text=NULL,error_code='owner_restarted',updated_at_ms=?1 WHERE status='submitting'", [now_ms])?;
            tx.execute("UPDATE station_codex_submission SET status='rejected',input_text=NULL,error_code='expired',updated_at_ms=?1 WHERE status='received' AND expires_at_ms<=?1", [now_ms])?;
            tx.execute("UPDATE station_codex_notification SET status=CASE WHEN channel='imessage' THEN 'awaiting_receipt' ELSE 'unknown' END,bounded_body=CASE WHEN channel='imessage' THEN bounded_body ELSE NULL END WHERE status='sending'", [])?;
            tx.execute("UPDATE station_codex_submission SET ack_status='unknown',ack_error_code='owner_restarted' WHERE ack_status='sending'", [])?;
            tx.execute("UPDATE station_imessage_receipt SET status='unknown',error_code='sender_restarted',updated_at_ms=?1 WHERE status='sending'", [now_ms])?;
            tx.execute("UPDATE station_codex_notification SET status=CASE WHEN status='awaiting_receipt' THEN 'unknown' ELSE 'expired' END,bounded_body=NULL WHERE expires_at_ms<=?1 AND status IN ('pending','awaiting_receipt')", [now_ms])?;
            tx.commit()
        })?;
        Ok(())
    }
}

pub(super) fn insert_submission(
    db: &rusqlite::Connection,
    r: &CodexSubmissionRow,
) -> rusqlite::Result<usize> {
    db.execute("INSERT INTO station_codex_submission(intent_id,principal,binding_id,binding_revision,operation_kind,expected_turn_id,source_kind,bot_id,chat_id,message_id,update_id,client_message_id,status,queue_id,turn_id,input_text,input_digest,created_at_ms,updated_at_ms,expires_at_ms,error_code,ack_status)
        VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,CASE WHEN ?7='telegram' THEN 'pending' ELSE 'suppressed' END) ON CONFLICT DO NOTHING",
        params![r.intent_id,r.principal,r.binding_id,r.binding_revision,r.operation_kind,r.expected_turn_id,r.source_kind,r.bot_id,r.chat_id,r.message_id,r.update_id,r.client_message_id,r.status,r.queue_id,r.turn_id,r.input_text,r.input_digest,r.created_at_ms,r.updated_at_ms,r.expires_at_ms,r.error_code])
}
const SUBMISSION_SELECT: &str="SELECT intent_id,principal,binding_id,binding_revision,operation_kind,expected_turn_id,source_kind,bot_id,chat_id,message_id,update_id,client_message_id,status,queue_id,turn_id,input_text,input_digest,created_at_ms,updated_at_ms,expires_at_ms,error_code FROM station_codex_submission";
fn submission_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<CodexSubmissionRow> {
    Ok(CodexSubmissionRow {
        intent_id: r.get(0)?,
        principal: r.get(1)?,
        binding_id: r.get(2)?,
        binding_revision: r.get(3)?,
        operation_kind: r.get(4)?,
        expected_turn_id: r.get(5)?,
        source_kind: r.get(6)?,
        bot_id: r.get(7)?,
        chat_id: r.get(8)?,
        message_id: r.get(9)?,
        update_id: r.get(10)?,
        client_message_id: r.get(11)?,
        status: r.get(12)?,
        queue_id: r.get(13)?,
        turn_id: r.get(14)?,
        input_text: r.get(15)?,
        input_digest: r.get(16)?,
        created_at_ms: r.get(17)?,
        updated_at_ms: r.get(18)?,
        expires_at_ms: r.get(19)?,
        error_code: r.get(20)?,
    })
}

fn insert_notification(
    db: &rusqlite::Connection,
    r: &CodexNotificationRow,
) -> rusqlite::Result<usize> {
    db.execute("INSERT INTO station_codex_notification(id,binding_id,binding_revision,turn_id,channel,destination_key,bot_id,chat_id,sender_host_id,sender_generation,status,provider_message_id,body_digest,bounded_body,created_at_ms,expires_at_ms,error_code) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17) ON CONFLICT DO NOTHING",params![r.id,r.binding_id,r.binding_revision,r.turn_id,r.channel,r.destination_key,r.bot_id,r.chat_id,r.sender_host_id,r.sender_generation,r.status,r.provider_message_id,r.body_digest,r.bounded_body,r.created_at_ms,r.expires_at_ms,r.error_code])
}
const NOTIFICATION_SELECT: &str="SELECT id,binding_id,binding_revision,turn_id,channel,destination_key,bot_id,chat_id,sender_host_id,sender_generation,status,provider_message_id,body_digest,bounded_body,created_at_ms,expires_at_ms,error_code FROM station_codex_notification";
fn notification_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<CodexNotificationRow> {
    Ok(CodexNotificationRow {
        id: r.get(0)?,
        binding_id: r.get(1)?,
        binding_revision: r.get(2)?,
        turn_id: r.get(3)?,
        channel: r.get(4)?,
        destination_key: r.get(5)?,
        bot_id: r.get(6)?,
        chat_id: r.get(7)?,
        sender_host_id: r.get(8)?,
        sender_generation: r.get(9)?,
        status: r.get(10)?,
        provider_message_id: r.get(11)?,
        body_digest: r.get(12)?,
        bounded_body: r.get(13)?,
        created_at_ms: r.get(14)?,
        expires_at_ms: r.get(15)?,
        error_code: r.get(16)?,
    })
}

fn decode_binding(r: &rusqlite::Row<'_>) -> rusqlite::Result<CodexBindingRow> {
    Ok(CodexBindingRow { id:r.get(0)?, owner_host_id:r.get(1)?, revision:r.get(2)?, session_key:r.get(3)?, endpoint_id:r.get(4)?, thread_id:r.get(5)?, enabled:r.get(6)?, telegram_destination_ref:r.get(7)?, imessage_enabled:r.get(8)?, bound_at_ms:r.get(9)? })
}
