use chrono::Utc;
use dandori_domain::{AuthContext, Workspace};
use sea_orm::{ActiveModelTrait, EntityTrait, Set, TransactionTrait};
use uuid::Uuid;

use crate::entities::workspace;
use crate::pg_store::{PgStore, WorkspaceWriteInput};
use crate::{StoreError, repositories::common::set_workspace_context_db};

pub(crate) async fn create_workspace(
    store: &PgStore,
    auth: &AuthContext,
    input: WorkspaceWriteInput,
) -> Result<Workspace, StoreError> {
    let tx = store.db().begin().await?;
    set_workspace_context_db(&tx, auth.workspace_id.0).await?;

    let model = workspace::ActiveModel {
        id: Set(input.workspace_id),
        name: Set(input.name),
        shard_bucket: Set(workspace::shard_bucket_for(input.workspace_id)),
        ..Default::default()
    }
    .insert(&tx)
    .await?;

    tx.commit().await?;
    Ok(map_workspace_model(model))
}

pub(crate) async fn get_workspace(
    store: &PgStore,
    auth: &AuthContext,
    workspace_id: Uuid,
) -> Result<Option<Workspace>, StoreError> {
    let tx = store.db().begin().await?;
    set_workspace_context_db(&tx, auth.workspace_id.0).await?;

    let model = workspace::Entity::find_by_id(workspace_id).one(&tx).await?;
    tx.commit().await?;

    Ok(model.map(map_workspace_model))
}

fn map_workspace_model(model: workspace::Model) -> Workspace {
    Workspace {
        id: model.id.into(),
        name: model.name,
        row_version: model.row_version,
        created_at: model.created_at.with_timezone(&Utc),
        updated_at: model.updated_at.with_timezone(&Utc),
    }
}
