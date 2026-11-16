use super::*;
use polar_dictate::{InputDetails, InputSnapshot, InputSummary};

impl SqliteDictateStore {
    pub(super) async fn write_input(&self, id: &str, snapshot: InputSnapshot) -> Result<()> {
        snapshot.validate()?;
        let owner = self.owner.clone();
        let id = id.to_owned();
        let content = serde_json::to_string(&snapshot.content)?;
        let digest = format!("{:x}", Sha256::digest(content.as_bytes()));
        let key = snapshot.key();
        self.db.transaction(move |tx| {
            let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM dictate_recording WHERE owner=?1 AND id=?2 UNION ALL SELECT 1 FROM dictate_text_edit WHERE owner=?1 AND id=?2)",params![owner,id],|r|r.get(0))?;
            if !exists {return Err(sql_error(anyhow::anyhow!("dictate.recording_missing")));}
            tx.execute("INSERT OR IGNORE INTO dictate_input_blob VALUES(?1,?2,?3,?4)",params![owner,id,digest,content])?;
            tx.execute("INSERT INTO dictate_input VALUES(?1,?2,?3,?4,?5,?6,?7,CAST(unixepoch('subsec')*1000 AS INTEGER),?8) ON CONFLICT(owner,operation_id,key) DO UPDATE SET status=excluded.status,digest=excluded.digest",
                params![owner,id,key,snapshot.request_id,snapshot.stage,snapshot.channel,snapshot.status,digest])?;
            Ok(())
        }).await?;
        Ok(())
    }
    pub(super) async fn read_inputs(&self, id: &str, key: Option<&str>) -> Result<InputDetails> {
        let owner = self.owner.clone();
        let id = id.to_owned();
        let key = key.map(str::to_owned);
        Ok(self.db.read(move |db| {
            let mut stmt=db.prepare("SELECT key,request_id,stage,channel,status,created_ms FROM dictate_input WHERE owner=?1 AND operation_id=?2 ORDER BY created_ms DESC,rowid DESC")?;
            let entries=stmt.query_map(params![owner,id],|r|Ok(InputSummary{key:r.get(0)?,request_id:r.get(1)?,stage:r.get(2)?,channel:r.get(3)?,status:r.get(4)?,created_ms:r.get::<_,i64>(5)?.max(0) as u64}))?.collect::<rusqlite::Result<Vec<_>>>()?;
            let selected_key=key.as_deref().or_else(||entries.first().map(|e|e.key.as_str()));
            let selected=if let Some(key)=selected_key {
                db.query_row("SELECT i.request_id,i.stage,i.channel,i.status,b.content FROM dictate_input i JOIN dictate_input_blob b ON b.owner=i.owner AND b.operation_id=i.operation_id AND b.digest=i.digest WHERE i.owner=?1 AND i.operation_id=?2 AND i.key=?3",params![owner,id,key],|r|{
                    let content:String=r.get(4)?;
                    Ok(InputSnapshot{request_id:r.get(0)?,stage:r.get(1)?,channel:r.get(2)?,status:r.get(3)?,content:serde_json::from_str(&content).map_err(|e|sql_error(e.into()))?})
                }).optional()?
            }else{None};
            Ok(InputDetails{entries,selected,error:None})
        }).await?)
    }
}
