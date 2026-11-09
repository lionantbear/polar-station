/// Canonical PCM export, shared by Host recovery and native local microphone tests.
pub fn pcm_wav(pcm: &[u8]) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(
        pcm.len().is_multiple_of(2) && pcm.len() as u64 <= crate::MAX_RECORDING_SAMPLES * 2,
        "dictate.invalid_audio"
    );
    let size = u32::try_from(pcm.len())?;
    let mut result = Vec::with_capacity(pcm.len() + 44);
    result.extend_from_slice(b"RIFF");
    result.extend_from_slice(&(size + 36).to_le_bytes());
    result.extend_from_slice(b"WAVEfmt ");
    result.extend_from_slice(&16u32.to_le_bytes());
    result.extend_from_slice(&1u16.to_le_bytes());
    result.extend_from_slice(&1u16.to_le_bytes());
    result.extend_from_slice(&16000u32.to_le_bytes());
    result.extend_from_slice(&32000u32.to_le_bytes());
    result.extend_from_slice(&2u16.to_le_bytes());
    result.extend_from_slice(&16u16.to_le_bytes());
    result.extend_from_slice(b"data");
    result.extend_from_slice(&size.to_le_bytes());
    result.extend_from_slice(pcm);
    Ok(result)
}
