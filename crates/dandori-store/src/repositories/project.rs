use chrono::Utc;
use dandori_domain::{AuthContext, Project};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set, TransactionTrait};
use uuid::Uuid;

use crate::entities::{project, workflow_version};
use crate::pg_store::{PgStore, ProjectWriteInput};
use crate::{StoreError, repositories::common::set_workspace_context_db};

pub(crate) async fn create_project(
    store: &PgStore,
    auth: &AuthContext,
    input: ProjectWriteInput,
) -> Result<Project, StoreError> {
    let tx = store.db().begin().await?;
    set_workspace_context_db(&tx, auth.workspace_id.0).await?;

    let workflow_exists = workflow_version::Entity::find()
        .filter(workflow_version::Column::WorkspaceId.eq(input.workspace_id))
        .filter(workflow_version::Column::Id.eq(input.workflow_version_id))
        .one(&tx)
        .await?
        .is_some();
    if !workflow_exists {
        return Err(StoreError::WorkflowVersionNotFound);
    }

    let model = project::ActiveModel {
        id: Set(input.project_id),
        workspace_id: Set(input.workspace_id),
        name: Set(input.name),
        workflow_version_id: Set(input.workflow_version_id),
        ..Default::default()
    }
    .insert(&tx)
    .await?;

    tx.commit().await?;
    Ok(map_project_model(model))
}

pub(crate) async fn get_project(
    store: &PgStore,
    auth: &AuthContext,
    project_id: Uuid,
) -> Result<Option<Project>, StoreError> {
    let tx = store.db().begin().await?;
    set_workspace_context_db(&tx, auth.workspace_id.0).await?;

    let model = project::Entity::find_by_id(project_id).one(&tx).await?;
    tx.commit().await?;

    Ok(model.map(map_project_model))
}

fn map_project_model(model: project::Model) -> Project {
    Project {
        id: model.id.into(),
        workspace_id: model.workspace_id.into(),
        name: model.name,
        workflow_version_id: model.workflow_version_id,
        row_version: model.row_version,
        created_at: model.created_at.with_timezone(&Utc),
        updated_at: model.updated_at.with_timezone(&Utc),
    }
}
