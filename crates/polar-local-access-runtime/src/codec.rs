use anyhow::{bail, Context, Result};
use serde::{de::DeserializeOwned, Serialize};
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
pub async fn read_json_frame<R, T>(reader: &mut R) -> Result<Option<T>>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut prefix = [0_u8; 4];
    match reader.read_exact(&mut prefix).await {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error).context("failed to read local IPC frame length"),
    }
    let length = u32::from_be_bytes(prefix) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        bail!("local IPC frame length {length} is outside 1..={MAX_FRAME_BYTES}");
    }
    let mut payload = vec![0_u8; length];
    reader
        .read_exact(&mut payload)
        .await
        .context("failed to read local IPC frame payload")?;
    serde_json::from_slice(&payload)
        .context("local IPC frame is not valid protocol JSON")
        .map(Some)
}

pub async fn write_json_frame<W, T>(writer: &mut W, frame: &T) -> Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let payload = serde_json::to_vec(frame).context("failed to encode local IPC frame")?;
    if payload.is_empty() || payload.len() > MAX_FRAME_BYTES {
        bail!(
            "encoded local IPC frame length {} is outside 1..={MAX_FRAME_BYTES}",
            payload.len()
        );
    }
    writer
        .write_all(&(payload.len() as u32).to_be_bytes())
        .await
        .context("failed to write local IPC frame length")?;
    writer
        .write_all(&payload)
        .await
        .context("failed to write local IPC frame payload")?;
    writer
        .flush()
        .await
        .context("failed to flush local IPC frame")
}
