use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseTransaction, Statement, Value};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::StoreError;

pub(super) async fn set_workspace_context_tx(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
) -> Result<(), StoreError> {
    sqlx::query("SELECT set_config('app.workspace_id', $1, true)")
        .bind(workspace_id.to_string())
        .execute(tx.as_mut())
        .await?;
    Ok(())
}

pub(super) async fn set_workspace_context_db(
    tx: &DatabaseTransaction,
    workspace_id: Uuid,
) -> Result<(), StoreError> {
    tx.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT set_config('app.workspace_id', $1, true)",
        vec![Value::String(Some(Box::new(workspace_id.to_string())))],
    ))
    .await?;
    Ok(())
}
