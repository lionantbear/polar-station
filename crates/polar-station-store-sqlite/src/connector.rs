//! Physical provider stream/receipt records in the registered Host database.
use crate::{SqliteCodexStore, StoreError};
use rusqlite::{params, OptionalExtension, TransactionBehavior};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorStreamRow {
    pub id: String,
    pub connector_id: String,
    pub destination_ref: String,
    pub thread_id: String,
    pub title: String,
    pub provider_scope: String,
    pub return_host_id: String,
    pub return_endpoint_id: String,
    pub status: String,
    pub platform_id: Option<String>,
    pub error_code: Option<String>,
    pub created_at_ms: i64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorDeliveryRow {
    pub id: String,
    pub stream_id: String,
    pub turn_id: String,
    pub event_kind: String,
    pub content_digest: String,
    pub body: Option<String>,
    pub status: String,
    pub provider_message_id: Option<String>,
    pub error_code: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub expires_at_ms: i64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorInboundRow {
    pub id: String,
    pub stream_id: String,
    pub provider_message_id: String,
    pub return_host_id: String,
    pub body: Option<String>,
    pub content_digest: String,
    pub status: String,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
}
fn stream_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ConnectorStreamRow> {
    Ok(ConnectorStreamRow {
        provider_scope: r.get(11)?,
        id: r.get(0)?,
        connector_id: r.get(1)?,
        destination_ref: r.get(2)?,
        thread_id: r.get(3)?,
        title: r.get(4)?,
        return_host_id: r.get(5)?,
        return_endpoint_id: r.get(6)?,
        status: r.get(7)?,
        platform_id: r.get(8)?,
        error_code: r.get(9)?,
        created_at_ms: r.get(10)?,
    })
}
fn delivery_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ConnectorDeliveryRow> {
    Ok(ConnectorDeliveryRow {
        id: r.get(0)?,
        stream_id: r.get(1)?,
        turn_id: r.get(2)?,
        event_kind: r.get(3)?,
        content_digest: r.get(4)?,
        body: r.get(5)?,
        status: r.get(6)?,
        provider_message_id: r.get(7)?,
        error_code: r.get(8)?,
        created_at_ms: r.get(9)?,
        updated_at_ms: r.get(10)?,
        expires_at_ms: r.get(11)?,
    })
}
fn inbound_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ConnectorInboundRow> {
    Ok(ConnectorInboundRow {
        id: r.get(0)?,
        stream_id: r.get(1)?,
        provider_message_id: r.get(2)?,
        return_host_id: r.get(3)?,
        body: r.get(4)?,
        content_digest: r.get(5)?,
        status: r.get(6)?,
        created_at_ms: r.get(7)?,
        expires_at_ms: r.get(8)?,
    })
}

impl SqliteCodexStore {
    /// One transaction admits all observer copies into one provider delivery.
    /// False reports conflicting content without changing the original route.
    pub fn admit_connector_delivery(
        &self,
        stream: ConnectorStreamRow,
        delivery: ConnectorDeliveryRow,
        source: String,
    ) -> Result<bool, StoreError> {
        self.database.runtime()?.write_blocking(move |db| {
            let tx=db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute("INSERT INTO station_connector_stream VALUES (?1,?2,?3,?4,?5,?6,?7,'pending',NULL,NULL,?8,?9) ON CONFLICT(id) DO NOTHING",
                params![stream.id,stream.connector_id,stream.destination_ref,stream.thread_id,stream.title,stream.return_host_id,stream.return_endpoint_id,stream.created_at_ms,stream.provider_scope])?;
            let same_scope:bool=tx.query_row("SELECT provider_scope=?2 AND connector_id=?3 AND destination_ref=?4 AND thread_id=?5 FROM station_connector_stream WHERE id=?1",params![stream.id,stream.provider_scope,stream.connector_id,stream.destination_ref,stream.thread_id],|r|r.get(0))?;
            if !same_scope {return Ok(false);}
            tx.execute("INSERT INTO station_connector_delivery VALUES (?1,?2,?3,?4,?5,?6,'pending',NULL,NULL,?7,?7,?8) ON CONFLICT(id) DO NOTHING",
                params![delivery.id,delivery.stream_id,delivery.turn_id,delivery.event_kind,delivery.content_digest,delivery.body,delivery.created_at_ms,delivery.expires_at_ms])?;
            let matches:bool=tx.query_row("SELECT content_digest=?2 AND stream_id=?3 AND turn_id=?4 AND event_kind=?5 FROM station_connector_delivery WHERE id=?1",
                params![delivery.id,delivery.content_digest,delivery.stream_id,delivery.turn_id,delivery.event_kind],|r|r.get(0))?;
            if !matches { return Ok(false); }
            tx.execute("INSERT INTO station_connector_producer VALUES (?1,?2) ON CONFLICT DO NOTHING",params![delivery.id,source])?;
            tx.commit()?; Ok(true)
        }).map_err(Into::into)
    }
    pub fn connector_stream(&self, id: &str) -> Result<Option<ConnectorStreamRow>, StoreError> {
        let id = id.to_owned();
        self.database
            .runtime()?
            .read_blocking(move |db| {
                db.query_row(
                    "SELECT * FROM station_connector_stream WHERE id=?1",
                    [id],
                    stream_row,
                )
                .optional()
            })
            .map_err(Into::into)
    }
    pub fn connector_stream_for_platform(
        &self,
        connector: &str,
        destination: &str,
        platform: &str,
    ) -> Result<Option<ConnectorStreamRow>, StoreError> {
        let (c, d, p) = (
            connector.to_owned(),
            destination.to_owned(),
            platform.to_owned(),
        );
        self.database.runtime()?.read_blocking(move |db|db.query_row("SELECT * FROM station_connector_stream WHERE connector_id=?1 AND destination_ref=?2 AND platform_id=?3 AND status='ready'",params![c,d,p],stream_row).optional()).map_err(Into::into)
    }
    pub fn connector_delivery(
        &self,
        id: &str,
        source: Option<&str>,
    ) -> Result<Option<ConnectorDeliveryRow>, StoreError> {
        let (id, source) = (id.to_owned(), source.map(str::to_owned));
        self.database.runtime()?.read_blocking(move |db|db.query_row("SELECT d.* FROM station_connector_delivery d WHERE d.id=?1 AND (?2 IS NULL OR EXISTS(SELECT 1 FROM station_connector_producer p WHERE p.delivery_id=d.id AND p.source_host_id=?2))",params![id,source],delivery_row).optional()).map_err(Into::into)
    }
    pub fn pending_connector_deliveries(
        &self,
        connector: &str,
        now: i64,
    ) -> Result<Vec<ConnectorDeliveryRow>, StoreError> {
        let connector = connector.to_owned();
        self.database.runtime()?.read_blocking(move |db| {
            let mut q=db.prepare("SELECT d.* FROM station_connector_delivery d JOIN station_connector_stream s ON s.id=d.stream_id WHERE s.connector_id=?1 AND d.status='pending' AND d.expires_at_ms>?2 ORDER BY d.created_at_ms,d.id LIMIT 32")?;
            let rows=q.query_map(params![connector,now],delivery_row)?;rows.collect()
        }).map_err(Into::into)
    }
    pub fn claim_connector_stream(&self, id: &str) -> Result<bool, StoreError> {
        let id = id.to_owned();
        Ok(self.database.runtime()?.write_blocking(move|db| db.execute("UPDATE station_connector_stream SET status='creating' WHERE id=?1 AND status='pending'",[id]))?==1)
    }
    pub fn finish_connector_stream(
        &self,
        id: &str,
        status: &str,
        platform: Option<String>,
        error: Option<String>,
    ) -> Result<bool, StoreError> {
        let (id, status) = (id.to_owned(), status.to_owned());
        Ok(self.database.runtime()?.write_blocking(move|db|db.execute("UPDATE station_connector_stream SET status=?2,platform_id=?3,error_code=?4 WHERE id=?1 AND status='creating'",params![id,status,platform,error]))?==1)
    }
    pub fn transition_connector_delivery(
        &self,
        id: &str,
        expected: &str,
        status: &str,
        provider: Option<String>,
        error: Option<String>,
        now: i64,
    ) -> Result<bool, StoreError> {
        let (id, expected, status) = (id.to_owned(), expected.to_owned(), status.to_owned());
        Ok(self.database.runtime()?.write_blocking(move|db|db.execute("UPDATE station_connector_delivery SET status=?3,provider_message_id=coalesce(?4,provider_message_id),error_code=?5,updated_at_ms=?6,body=CASE WHEN ?3 IN ('sent','failed','unknown','expired') THEN NULL ELSE body END WHERE id=?1 AND status=?2",params![id,expected,status,provider,error,now]))?==1)
    }
    /// Shares the provider cursor transaction with legacy admission. A mixed
    /// update batch cannot acknowledge a topic message before it is persisted.
    pub fn accept_connector_telegram_batch(
        &self,
        bot: i64,
        next: i64,
        now: i64,
        rows: Vec<ConnectorInboundRow>,
        legacy: Vec<crate::CodexSubmissionRow>,
    ) -> Result<(), StoreError> {
        self.database.runtime()?.write_blocking(move|db| {
            let tx=db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            for r in rows {
                tx.execute("INSERT INTO station_connector_inbound VALUES (?1,?2,?3,?4,?5,?6,'received',?7,?8) ON CONFLICT DO NOTHING",params![r.id,r.stream_id,r.provider_message_id,r.return_host_id,r.body,r.content_digest,r.created_at_ms,r.expires_at_ms])?;
            }
            for row in legacy {super::codex_control::insert_submission(&tx,&row)?;}
            tx.execute("INSERT INTO station_telegram_cursor VALUES (?1,?2,?3) ON CONFLICT(bot_id) DO UPDATE SET next_offset=max(next_offset,excluded.next_offset),updated_at_ms=excluded.updated_at_ms",params![bot,next,now])?;
            tx.commit()
        })?;
        Ok(())
    }
    pub fn connector_inbound(
        &self,
        connector: &str,
        host: &str,
        now: i64,
    ) -> Result<Vec<ConnectorInboundRow>, StoreError> {
        let (connector, host) = (connector.to_owned(), host.to_owned());
        self.database.runtime()?.read_blocking(move|db| {
            let mut q=db.prepare("SELECT i.* FROM station_connector_inbound i JOIN station_connector_stream s ON s.id=i.stream_id WHERE s.connector_id=?1 AND i.return_host_id=?2 AND i.status='received' AND i.expires_at_ms>?3 ORDER BY i.created_at_ms,i.id LIMIT 1")?;
            let rows=q.query_map(params![connector,host,now],inbound_row)?;rows.collect()
        }).map_err(Into::into)
    }
    pub fn acknowledge_connector_inbound(
        &self,
        id: &str,
        connector: &str,
        host: &str,
        status: &str,
    ) -> Result<bool, StoreError> {
        let (id, connector, host, status) = (
            id.to_owned(),
            connector.to_owned(),
            host.to_owned(),
            status.to_owned(),
        );
        Ok(self.database.runtime()?.write_blocking(move|db|db.execute("UPDATE station_connector_inbound SET status=?4,body=NULL WHERE id=?1 AND return_host_id=?3 AND status='received' AND EXISTS(SELECT 1 FROM station_connector_stream s WHERE s.id=stream_id AND s.connector_id=?2)",params![id,connector,host,status]))?==1)
    }
    pub fn interrupt_connector_io(&self, connector: &str, now: i64) -> Result<(), StoreError> {
        let connector = connector.to_owned();
        self.database.runtime()?.write_blocking(move|db| {
            let tx=db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute("UPDATE station_connector_stream SET status='unknown',error_code='creation_interrupted' WHERE connector_id=?1 AND status='creating'",[&connector])?;
            tx.execute("UPDATE station_connector_delivery SET status='unknown',body=NULL,error_code='send_interrupted',updated_at_ms=?2 WHERE status='sending' AND stream_id IN (SELECT id FROM station_connector_stream WHERE connector_id=?1)",params![connector,now])?;
            tx.commit()
        })?;
        Ok(())
    }
    pub fn connector_streams(
        &self,
        connector: &str,
    ) -> Result<Vec<ConnectorStreamRow>, StoreError> {
        let connector = connector.to_owned();
        self.database.runtime()?.read_blocking(move|db| {
            let mut q=db.prepare("SELECT * FROM station_connector_stream WHERE connector_id=?1 ORDER BY created_at_ms DESC,id LIMIT 100")?;
            let rows=q.query_map([connector],stream_row)?;rows.collect()
        }).map_err(Into::into)
    }
    pub fn change_connector_return_route(
        &self,
        id: &str,
        expected_host: &str,
        host: &str,
        endpoint: &str,
    ) -> Result<bool, StoreError> {
        let (id, expected, host, endpoint) = (
            id.to_owned(),
            expected_host.to_owned(),
            host.to_owned(),
            endpoint.to_owned(),
        );
        Ok(self.database.runtime()?.write_blocking(move|db|db.execute("UPDATE station_connector_stream SET return_host_id=?3,return_endpoint_id=?4 WHERE id=?1 AND return_host_id=?2 AND NOT EXISTS(SELECT 1 FROM station_connector_inbound WHERE stream_id=?1 AND status IN ('received','unknown'))",params![id,expected,host,endpoint]))?==1)
    }
    pub fn recover_connectors(&self, now: i64) -> Result<(), StoreError> {
        self.database.runtime()?.write_blocking(move|db| {
            let tx=db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute("UPDATE station_connector_stream SET status='unknown',error_code='creation_interrupted' WHERE status='creating'",[])?;
            tx.execute("UPDATE station_connector_delivery SET status='unknown',body=NULL,error_code='send_interrupted',updated_at_ms=?1 WHERE status='sending'",[now])?;
            tx.commit()
        })?;
        self.expire_connector_text(now)
    }
    pub fn expire_connector_text(&self, now: i64) -> Result<(), StoreError> {
        self.database.runtime()?.write_blocking(move|db| {
            let tx=db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute("UPDATE station_connector_delivery SET status='expired',body=NULL,updated_at_ms=?1 WHERE status='pending' AND expires_at_ms<=?1",[now])?;
            tx.execute("UPDATE station_connector_inbound SET status='expired',body=NULL WHERE status='received' AND expires_at_ms<=?1",[now])?;
            tx.commit()
        })?;
        Ok(())
    }
}
