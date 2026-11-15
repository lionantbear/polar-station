use super::*;
use anyhow::Context;

impl PolarVault {
    /// Initialize once, including concurrent creation attempts. Never rotate identity.
    pub async fn initialize_inhouse_polar_identity_seed(
        &self,
        inhouse_polar_id: &str,
        seed: IdentitySeed,
    ) -> Result<()> {
        let (nonce, ciphertext) = self
            .cipher
            .seal(hex::encode(seed.as_bytes()).as_bytes())
            .await?;
        self.store
            .insert_secret_material(
                "inhouse_polar.identity_seed".into(),
                inhouse_polar_id.into(),
                nonce.to_vec(),
                ciphertext,
            )
            .await
    }

    pub async fn put_inhouse_polar_identity_seed(
        &self,
        inhouse_polar_id: &str,
        seed: IdentitySeed,
    ) -> Result<()> {
        self.put_secret_material(
            "inhouse_polar.identity_seed",
            inhouse_polar_id,
            &hex::encode(seed.as_bytes()),
        )
        .await
    }

    pub async fn get_inhouse_polar_identity_seed(
        &self,
        inhouse_polar_id: &str,
    ) -> Result<Option<IdentitySeed>> {
        let Some(raw) = self
            .get_secret_material("inhouse_polar.identity_seed", inhouse_polar_id)
            .await?
        else {
            return Ok(None);
        };
        let bytes = hex::decode(raw.trim()).context("decode inhouse polar identity seed")?;
        let seed_bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| anyhow!("invalid inhouse polar identity seed length"))?;
        Ok(Some(IdentitySeed::from_bytes(seed_bytes)))
    }
}
