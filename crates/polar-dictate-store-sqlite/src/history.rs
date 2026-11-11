use super::*;
use polar_dictate::{
    CleanupReport, HistoryCursor, HistoryItem, HistoryPage, ItemChange, RetentionPolicy,
    TextEditRecord,
};

pub(super) fn index_record(db: &Connection, owner: &str, r: &Recording) -> Result<()> {
    let pending: bool = db.query_row(
        "SELECT legacy_pending FROM dictate_recording WHERE owner=?1 AND id=?2",
        params![owner, r.id],
        |row| row.get(0),
    )?;
    index_record_with_pending(db, owner, r, pending)
}
fn index_record_with_pending(
    db: &Connection,
    owner: &str,
    r: &Recording,
    legacy_pending: bool,
) -> Result<()> {
    let item = HistoryItem::Recording(Box::new(r.clone()));
    let work = r.processing_enabled
        && (r
            .segments
            .iter()
            .any(|s| s.state == polar_dictate::SegmentState::Pending)
            || (r.transcription_mode == polar_dictate::TranscriptionMode::Realtime
                && r.capture == CaptureState::Recording)
            || (r.original().is_some()
                && r.cleanup_enabled
                && !r.cleanup_deferred
                && r.cleaned.is_none()
                && r.cleanup_error.is_none()
                && !r.cleanup_running));
    let summary = item.summary();
    let visible = !summary.is_test && (summary.has_text || summary.has_audio);
    let recoverable = work
        || r.capture == CaptureState::Recording
        || r.cleanup_running
        || r.delivery == polar_dictate::DeliveryState::Prepared;
    db.execute("UPDATE dictate_recording SET summary=?3,work_pending=?4,recoverable=?5,protected=?6,completed_ms=?7,audio_bytes=?8,visible=?9 WHERE owner=?1 AND id=?2",
        params![owner,r.id,serde_json::to_string(&item.summary())?,work,recoverable,item.protected() || legacy_pending,r.completed_at_ms.map(i64::try_from).transpose()?,i64::try_from(item.audio_bytes())?,visible])?;
    Ok(())
}
pub(super) fn backfill(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    // One-time declared migration, in bounded batches; not an application-open scan.
    let mut after = String::new();
    loop {
        let mut stmt=tx.prepare("SELECT owner,id,payload FROM dictate_recording WHERE summary IS NULL AND owner||char(0)||id>?1 ORDER BY owner,id LIMIT 200")?;
        let rows = stmt
            .query_map([&after], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if rows.is_empty() {
            break;
        }
        for (owner, id, payload) in rows {
            let r: Recording = serde_json::from_str(&payload).map_err(|e| sql_error(e.into()))?;
            index_record_with_pending(tx, &owner, &r, false).map_err(sql_error)?;
            after = format!("{owner}\0{id}");
        }
    }
    Ok(())
}
fn get_text(db: &Connection, owner: &str, id: &str) -> Result<TextEditRecord> {
    let payload: String = db
        .query_row(
            "SELECT payload FROM dictate_text_edit WHERE owner=?1 AND id=?2",
            params![owner, id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| anyhow::anyhow!("dictate.recording_missing"))?;
    Ok(serde_json::from_str(&payload)?)
}
fn item(db: &Connection, owner: &str, id: &str) -> Result<HistoryItem> {
    let exists: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM dictate_recording WHERE owner=?1 AND id=?2)",
        params![owner, id],
        |r| r.get(0),
    )?;
    if exists {
        Ok(HistoryItem::Recording(Box::new(get(db, owner, id)?)))
    } else {
        Ok(HistoryItem::Text(Box::new(get_text(db, owner, id)?)))
    }
}
fn save_text(db: &Connection, owner: &str, r: &TextEditRecord) -> Result<()> {
    let item = HistoryItem::Text(Box::new(r.clone()));
    db.execute("INSERT INTO dictate_text_edit(owner,id,created_ms,payload,summary,protected,completed_ms,running) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(owner,id) DO UPDATE SET payload=excluded.payload,summary=excluded.summary,protected=excluded.protected,completed_ms=excluded.completed_ms,running=excluded.running",
        params![owner,r.id,i64::try_from(r.created_at_ms.unwrap_or(0))?,serde_json::to_string(r)?,serde_json::to_string(&item.summary())?,r.protected(),r.completed_at_ms.map(i64::try_from).transpose()?,r.running])?;
    Ok(())
}
impl SqliteDictateStore {
    pub(super) async fn create_text(&self, r: TextEditRecord) -> Result<TextEditRecord> {
        let owner = self.owner.clone();
        Ok(self
            .db
            .transaction(move |tx| {
                (|| -> Result<_> {
                    match item(tx, &owner, &r.id) {
                        Ok(HistoryItem::Text(old)) => {
                            ensure!(
                                old.original == r.original && old.mode == r.mode,
                                "dictate.text_id_conflict"
                            );
                            return Ok(*old);
                        }
                        Ok(_) => anyhow::bail!("dictate.text_id_conflict"),
                        Err(e) if e.to_string() == "dictate.recording_missing" => {}
                        Err(e) => return Err(e),
                    }
                    save_text(tx, &owner, &r)?;
                    Ok(r)
                })()
                .map_err(sql_error)
            })
            .await?)
    }
    pub(super) async fn finish_text(
        &self,
        id: &str,
        result: Result<String, String>,
    ) -> Result<TextEditRecord> {
        let (owner, id) = (self.owner.clone(), id.to_owned());
        Ok(self
            .db
            .transaction(move |tx| {
                (|| -> Result<_> {
                    let mut r = get_text(tx, &owner, &id)?;
                    if !r.running {
                        return Ok(r);
                    }
                    match result {
                        Ok(text) => {
                            ensure!(
                                polar_dictate::history::valid_text(&text),
                                "text.edit_invalid_result"
                            );
                            r.result = Some(text);
                            r.error = None;
                        }
                        Err(error) => r.error = Some(error.chars().take(512).collect()),
                    }
                    r.running = false;
                    r.revision += 1;
                    save_text(tx, &owner, &r)?;
                    Ok(r)
                })()
                .map_err(sql_error)
            })
            .await?)
    }
    pub(super) async fn get_item(&self, id: &str) -> Result<HistoryItem> {
        let (owner, id) = (self.owner.clone(), id.to_owned());
        Ok(self
            .db
            .read(move |db| item(db, &owner, &id).map_err(sql_error))
            .await?)
    }
    pub(super) async fn change_item(&self, id: &str, change: ItemChange) -> Result<HistoryItem> {
        let (owner, id) = (self.owner.clone(), id.to_owned());
        Ok(self
            .db
            .transaction(move |tx| {
                (|| -> Result<_> {
                    let mut item = item(tx, &owner, &id)?;
                    match &mut item {
                        HistoryItem::Text(r) => {
                            r.apply(change)?;
                            save_text(tx, &owner, r)?;
                        }
                        HistoryItem::Recording(r) => {
                            match change {
                                ItemChange::UseAvailable => {
                                    let text = r.text().filter(|s| !s.trim().is_empty())
                                        .or_else(|| { let text=r.receipt().transcript_preview; (!text.trim().is_empty()).then_some(text) });
                                    r.apply(Change::UseAvailable { text })?;
                                }
                                ItemChange::Edit { text } => {
                                    ensure!(
                                        !item_record_active(r)
                                            && r.delivery != polar_dictate::DeliveryState::Prepared
                                            && text.len() <= polar_dictate::MAX_TEXT_BYTES
                                            && !text.contains('\0'),
                                        "dictate.text_edit_not_ready"
                                    );
                                    r.draft_text = Some(text);
                                    r.completed_at_ms = None;
                                    if r.delivery != polar_dictate::DeliveryState::Unknown {
                                        r.delivery = polar_dictate::DeliveryState::Pending;
                                    }
                                    r.dismissed = false;
                                    r.revision += 1;
                                }
                                ItemChange::Prepare {
                                    token,
                                    text,
                                    edited,
                                } => {
                                    r.apply(Change::PrepareTextDelivery {
                                        token,
                                        text: text.clone(),
                                        edited,
                                    })?;
                                    if edited {
                                        let prefix =
                                            polar_dictate::quoted_transcript(&r.reference, "");
                                        let body = if r.reference.is_empty() {
                                            text.as_str()
                                        } else {
                                            text.strip_prefix(&prefix).ok_or_else(|| {
                                                anyhow::anyhow!("dictate.delivery_quote_changed")
                                            })?
                                        };
                                        ensure!(
                                            body.len() <= polar_dictate::MAX_TEXT_BYTES,
                                            "dictate.invalid_draft"
                                        );
                                        r.draft_text = Some(body.into());
                                    }
                                }
                                ItemChange::Complete {
                                    token,
                                    applied,
                                    now_ms,
                                } => r.apply(Change::CompleteDelivery {
                                    token,
                                    applied,
                                    now_ms,
                                })?,
                                ItemChange::Copied { now_ms, text } => {
                                    if r.delivery != polar_dictate::DeliveryState::Delivered {
                                        r.apply(Change::MarkCopied { now_ms })?;
                                    }
                                    if let Some(text) = text {
                                        ensure!(
                                            !text.trim().is_empty()
                                                && text.len() <= 2 * polar_dictate::MAX_TEXT_BYTES
                                                && !text.contains('\0'),
                                            "dictate.invalid_copy_text"
                                        );
                                        r.copy_receipt =
                                            Some(polar_dictate::history::CopyReceipt {
                                                text,
                                                at_ms: now_ms,
                                            });
                                        r.revision += 1;
                                    }
                                }
                                ItemChange::Reopen => {
                                    r.apply(Change::ReopenCopy)?;
                                    r.completed_at_ms = None;
                                }
                                ItemChange::Pin { pinned } => r.apply(Change::Pin { pinned })?,
                                ItemChange::Dismiss => {
                                    ensure!(
                                        !item_record_active(r)
                                            && r.delivery != polar_dictate::DeliveryState::Prepared,
                                        "dictate.stop_before_dismiss"
                                    );
                                    r.dismissed = true;
                                    r.revision += 1;
                                }
                            }
                            update(tx, &owner, r)?;
                        }
                    }
                    Ok(item)
                })()
                .map_err(sql_error)
            })
            .await?)
    }
    pub(super) async fn history_page(
        &self,
        cursor: Option<HistoryCursor>,
        limit: u32,
    ) -> Result<HistoryPage> {
        let owner = self.owner.clone();
        let limit = limit.clamp(1, 100);
        if let Some(c) = &cursor {
            ensure!(
                c.kind <= 1 && c.created_ms <= i64::MAX as u64 && polar_dictate::identifier(&c.id),
                "dictate.invalid_history_cursor"
            );
        }
        let (at, kind, id) = cursor.map(|c| (c.created_ms, c.kind, c.id)).unwrap_or((
            i64::MAX as u64,
            2,
            String::new(),
        ));
        let rows=self.db.read(move |db|{
            let mut stmt=db.prepare("SELECT created_ms,kind,id,summary FROM (SELECT created_ms,0 kind,id,summary FROM dictate_recording WHERE owner=?1 AND visible=1 AND (created_ms,0,id)<(?2,?3,?4) UNION ALL SELECT created_ms,1 kind,id,summary FROM dictate_text_edit WHERE owner=?1 AND (created_ms,1,id)<(?2,?3,?4)) ORDER BY created_ms DESC,kind DESC,id DESC LIMIT ?5")?;
            let rows=stmt.query_map(params![owner,at as i64,kind,id,limit+1],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,u8>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?)))?.collect::<rusqlite::Result<Vec<_>>>();rows
        }).await?;
        let more = rows.len() > limit as usize;
        let rows = &rows[..rows.len().min(limit as usize)];
        let next = if more {
            rows.last().map(|(at, kind, id, _)| HistoryCursor {
                created_ms: *at as u64,
                kind: *kind,
                id: id.clone(),
            })
        } else {
            None
        };
        Ok(HistoryPage {
            items: rows
                .iter()
                .map(|(_, _, _, s)| serde_json::from_str(s))
                .collect::<Result<_, _>>()?,
            next,
        })
    }
    pub(super) async fn cleanup_history(
        &self,
        now_ms: u64,
        policy: RetentionPolicy,
        apply: bool,
    ) -> Result<CleanupReport> {
        policy.validate()?;
        let owner = self.owner.clone();
        let cutoff = now_ms
            .checked_sub(policy.duration_ms())
            .map(|t| t.min(i64::MAX as u64) as i64)
            .unwrap_or(-1);
        let mut deleted = 0;
        let mut audio_bytes = 0;
        if apply && policy.enabled {
            for table in ["dictate_recording", "dictate_text_edit"] {
                loop {
                    let owner = owner.clone();
                    let (count,bytes)=self.db.transaction(move |tx|{
                        let audio=if table=="dictate_recording"{"audio_bytes"}else{"0"};
                        // One audio record per transaction can already contain
                        // ten minutes of PCM. Yield between records through the
                        // existing SQLite actor so capture writes remain fair.
                        let batch=if table=="dictate_recording"{1}else{100};
                        let mut stmt=tx.prepare(&format!("SELECT id,{audio} FROM {table} WHERE owner=?1 AND protected=0 AND completed_ms<=?2 ORDER BY completed_ms,id LIMIT ?3"))?;
                        let rows=stmt.query_map(params![owner,cutoff,batch],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
                        let count=rows.len();let bytes=rows.iter().map(|(_,bytes)|bytes).sum::<i64>();
                        for (id,_) in rows {
                            mark_deleted(tx,&owner,&id).map_err(sql_error)?;
                            if table=="dictate_recording" {tx.execute("DELETE FROM dictate_audio_block WHERE owner=?1 AND recording_id=?2",params![owner,id])?;}
                            tx.execute(&format!("DELETE FROM {table} WHERE owner=?1 AND id=?2"),params![owner,id])?;
                        }
                        Ok((count,bytes))
                    }).await?;
                    deleted += count as u64;
                    audio_bytes += u64::try_from(bytes)?;
                    if count == 0 {
                        break;
                    }
                }
            }
        }
        let mut report = self
            .db
            .read(move |db| inspect_cleanup(db, &owner, cutoff, policy).map_err(sql_error))
            .await?;
        if apply {
            report.deleted = deleted;
            report.audio_bytes = audio_bytes;
        }
        Ok(report)
    }
    pub(super) async fn pending_records(&self) -> Result<Vec<Recording>> {
        let owner = self.owner.clone();
        let payloads=self.db.read(move |db|{
            let mut stmt=db.prepare("SELECT payload FROM dictate_recording WHERE owner=?1 AND work_pending=1 ORDER BY created_ms,id LIMIT 200")?;
            let rows=stmt.query_map([owner],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>();rows
        }).await?;
        Ok(payloads
            .iter()
            .map(|s| serde_json::from_str(s))
            .collect::<Result<_, _>>()?)
    }
}
fn item_record_active(r: &Recording) -> bool {
    r.capture == CaptureState::Recording
        || r.cleanup_running
        || r.segments
            .iter()
            .any(|s| s.state == polar_dictate::SegmentState::Running)
}

pub(super) fn mark_deleted(db: &Connection, owner: &str, id: &str) -> Result<()> {
    // A typed deletion decision in the existing import ledger carries no text.
    // Empty digest is reserved for this decision; imported snapshots use SHA256.
    db.execute(
        "UPDATE dictate_legacy_import SET payload='',conflict=0 WHERE owner=?1 AND id=?2",
        params![owner, id],
    )?;
    db.execute("INSERT INTO dictate_legacy_import(owner,id,digest,conflict,payload,deleted) VALUES(?1,?2,'',0,'',1) ON CONFLICT(owner,id,digest) DO UPDATE SET deleted=1,payload=''",params![owner,id])?;
    Ok(())
}
pub(super) fn delete_text(db: &Connection, owner: &str, id: &str) -> Result<bool> {
    let found: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM dictate_text_edit WHERE owner=?1 AND id=?2)",
        params![owner, id],
        |r| r.get(0),
    )?;
    if !found {
        return Ok(false);
    }
    let r = get_text(db, owner, id)?;
    ensure!(
        !r.running && r.delivery != polar_dictate::DeliveryState::Prepared,
        "dictate.stop_before_delete"
    );
    mark_deleted(db, owner, id)?;
    db.execute(
        "DELETE FROM dictate_text_edit WHERE owner=?1 AND id=?2",
        params![owner, id],
    )?;
    Ok(true)
}
pub(super) fn recover_texts(db: &Connection, owner: &str) -> Result<usize> {
    let mut statement =
        db.prepare("SELECT payload FROM dictate_text_edit WHERE owner=?1 AND running=1")?;
    let rows = statement
        .query_map([owner], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let count = rows.len();
    for row in rows {
        let mut r: TextEditRecord = serde_json::from_str(&row)?;
        r.running = false;
        r.error = Some("Refinement interrupted; original retained".into());
        r.revision += 1;
        save_text(db, owner, &r)?;
    }
    Ok(count)
}

impl SqliteDictateStore {
    pub(super) async fn release_legacy_barrier(&self) -> Result<()> {
        loop {
            let owner = self.owner.clone();
            let count=self.db.transaction(move |tx|(||->Result<usize>{
                let mut query=tx.prepare("SELECT id FROM dictate_recording r WHERE owner=?1 AND legacy_pending=1 AND NOT EXISTS(SELECT 1 FROM dictate_legacy_import i WHERE i.owner=r.owner AND i.id=r.id AND i.conflict=1) LIMIT 200")?;
                let ids=query.query_map([&owner],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
                let count=ids.len();
                for id in ids {tx.execute("UPDATE dictate_recording SET legacy_pending=0 WHERE owner=?1 AND id=?2",params![owner,id])?;index_record(tx,&owner,&get(tx,&owner,&id)?)?;}
                Ok(count)
            })().map_err(sql_error)).await?;
            if count < 200 {
                break;
            }
        }
        Ok(())
    }
    pub(super) async fn import_inputs(
        &self,
        entries: Vec<polar_dictate::LegacyInput>,
    ) -> Result<polar_dictate::ImportReport> {
        let owner = self.owner.clone();
        Ok(self.db.transaction(move |tx|(||->Result<_>{
            let mut report=polar_dictate::ImportReport::default();
            for entry in entries {
                ensure!(polar_dictate::identifier(&entry.operation_id)&&entry.raw.len()<=polar_dictate::MAX_TEXT_BYTES&&entry.text.len()<=polar_dictate::MAX_TEXT_BYTES&&!entry.raw.contains('\0')&&!entry.text.contains('\0'),"dictate.invalid_legacy_input");
                let deleted:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM dictate_legacy_import WHERE owner=?1 AND id=?2 AND deleted=1)",params![owner,entry.operation_id],|row|row.get(0))?;
                if deleted {continue;}
                let payload=serde_json::to_string(&entry)?;let digest=format!("{:x}",Sha256::digest(payload.as_bytes()));
                let seen:Option<bool>=tx.query_row("SELECT conflict FROM dictate_legacy_import WHERE owner=?1 AND id=?2 AND digest=?3",params![owner,entry.operation_id,digest],|r|r.get(0)).optional()?;
                if let Some(conflict)=seen {if conflict {report.conflicts.push(entry.operation_id);}continue;}
                let mut conflict=false;
                match item(tx,&owner,&entry.operation_id){
                    Ok(HistoryItem::Recording(mut r))=>{
                        if entry.edited && r.draft_text.as_ref().is_some_and(|text|text!=&entry.text){conflict=true;}
                        else {
                            if entry.edited {r.draft_text=Some(entry.text.clone());if !matches!(entry.phase.as_str(),"delivered"|"copied"){r.completed_at_ms=None;}}
                            if entry.phase=="unknown" && r.delivery==polar_dictate::DeliveryState::Pending {r.delivery=polar_dictate::DeliveryState::Unknown;}
                            if matches!(entry.phase.as_str(),"delivered"|"copied") && entry.edited {r.delivery_text=Some(polar_dictate::quoted_transcript(&entry.reference,&entry.text));}
                            r.revision=r.revision.checked_add(1).ok_or_else(||anyhow::anyhow!("dictate.revision_exhausted"))?;
                            tx.execute("UPDATE dictate_recording SET legacy_pending=0 WHERE owner=?1 AND id=?2",params![owner,r.id])?;
                            update(tx,&owner,&r)?;
                        }
                    }
                    Ok(HistoryItem::Text(r))=>{conflict=r.original!=entry.raw || (entry.edited && r.text()!=entry.text);}
                    Err(e) if e.to_string()=="dictate.recording_missing"=>{
                        if !entry.raw.trim().is_empty()||!entry.text.trim().is_empty(){
                            let original=if entry.raw.trim().is_empty(){entry.text.clone()}else{entry.raw.clone()};
                            let mode=if entry.output_mode==Some(polar_dictate::DictateOutputMode::Transcreate){polar_dictate::RefineMode::English}else{polar_dictate::RefineMode::Cleanup};
                            let mut r=TextEditRecord::new(entry.operation_id.clone(),original,mode,entry.created_at_ms.unwrap_or(0))?;
                            r.output_mode=entry.output_mode;r.created_at_ms=entry.created_at_ms;r.running=false;r.import_digest=Some(digest.clone());
                            r.result=entry.refine_baseline.clone().or_else(||entry.refined.then(||entry.text.clone()));
                            if entry.edited {r.draft_text=Some(entry.text.clone());}
                            else if r.result.is_none() && !entry.text.trim().is_empty(){r.result=Some(entry.text.clone());}
                            r.error=entry.error.clone();
                            r.delivery=match entry.phase.as_str(){"unknown"|"delivering"=>polar_dictate::DeliveryState::Unknown,"delivered"=>polar_dictate::DeliveryState::Delivered,"copied"=>polar_dictate::DeliveryState::Copied,_=>polar_dictate::DeliveryState::Pending};
                            // Legacy snapshots have no trustworthy completion time.
                            // Retain them until an explicit new completion decision.
                            save_text(tx,&owner,&r)?;
                        }
                    }
                    Err(e)=>return Err(e),
                }
                tx.execute("INSERT INTO dictate_legacy_import(owner,id,digest,conflict,payload) VALUES(?1,?2,?3,?4,?5)",params![owner,entry.operation_id,digest,conflict,if conflict{payload}else{String::new()}])?;
                if conflict {report.conflicts.push(entry.operation_id);}else{report.imported+=1;}
            }
            Ok(report)
        })().map_err(sql_error)).await?)
    }
}

fn inspect_cleanup(
    db: &Connection,
    owner: &str,
    cutoff: i64,
    policy: RetentionPolicy,
) -> Result<CleanupReport> {
    let mut report = CleanupReport::default();
    for table in ["dictate_recording", "dictate_text_edit"] {
        report.protected += u64::try_from(db.query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE owner=?1 AND protected=1"),
            [owner],
            |r| r.get::<_, i64>(0),
        )?)?;
        if !policy.enabled {
            continue;
        }
        let audio = if table == "dictate_recording" {
            "audio_bytes"
        } else {
            "0"
        };
        let (count,bytes):(i64,i64)=db.query_row(&format!("SELECT COUNT(*),COALESCE(SUM({audio}),0) FROM {table} WHERE owner=?1 AND protected=0 AND completed_ms<=?2"),params![owner,cutoff],|r|Ok((r.get(0)?,r.get(1)?)))?;
        report.eligible += u64::try_from(count)?;
        report.audio_bytes += u64::try_from(bytes)?;
        let next: Option<i64> = db.query_row(
            &format!("SELECT MIN(completed_ms) FROM {table} WHERE owner=?1 AND protected=0"),
            [owner],
            |r| r.get(0),
        )?;
        if let Some(at) = next {
            let at = u64::try_from(at)?.saturating_add(policy.duration_ms());
            report.next_at_ms = Some(report.next_at_ms.map_or(at, |old| old.min(at)));
        }
    }
    Ok(report)
}
