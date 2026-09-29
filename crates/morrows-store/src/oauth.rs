use super::*;

impl Store {
    /// Lock before reading so independent server processes cannot redeem the same
    /// code or refresh token concurrently. Even OAuth error outcomes are committed:
    /// refresh-token reuse must persist revocation of the entire token family.
    pub async fn update_oauth<T>(
        &self,
        update: impl FnOnce(&mut Value) -> T + Send,
    ) -> Result<T, DomainError> {
        let mut tx = self.pool.begin().await.map_err(storage)?;
        sqlx::query("UPDATE morrows_oauth_state SET state=state WHERE id=1")
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        let raw: String = sqlx::query_scalar("SELECT state FROM morrows_oauth_state WHERE id=1")
            .fetch_one(&mut *tx)
            .await
            .map_err(storage)?;
        let mut state = serde_json::from_str(&raw).map_err(storage)?;
        let result = update(&mut state);
        sqlx::query("UPDATE morrows_oauth_state SET state=? WHERE id=1")
            .bind(serde_json::to_string(&state).map_err(storage)?)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        tx.commit().await.map_err(storage)?;
        Ok(result)
    }
}
