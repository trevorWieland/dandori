pub mod workspace {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "workspace")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub name: String,
        pub shard_bucket: i16,
        pub row_version: i64,
        pub created_at: DateTimeWithTimeZone,
        pub updated_at: DateTimeWithTimeZone,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}

    /// Deterministic bucket assignment from a workspace UUID.
    ///
    /// Uses the trailing 16 bits of the v4/v7 UUID (which are uniformly
    /// random for v4 and include a random tail for v7), clamped to the
    /// 0..=1023 range that matches the migration check constraint.
    #[must_use]
    pub fn shard_bucket_for(id: Uuid) -> i16 {
        let bytes = id.as_bytes();
        let tail = u16::from_be_bytes([bytes[14], bytes[15]]);
        (tail % 1024) as i16
    }
}

pub mod workflow_version {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "workflow_version")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub workspace_id: Uuid,
        pub name: String,
        pub version: i32,
        pub checksum: String,
        pub states: Json,
        pub transitions: Json,
        pub created_at: DateTimeWithTimeZone,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod project {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "project")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub workspace_id: Uuid,
        pub name: String,
        pub workflow_version_id: Uuid,
        pub row_version: i64,
        pub created_at: DateTimeWithTimeZone,
        pub updated_at: DateTimeWithTimeZone,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod milestone {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "milestone")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub workspace_id: Uuid,
        pub project_id: Uuid,
        pub title: String,
        pub due_at: Option<DateTimeWithTimeZone>,
        pub row_version: i64,
        pub created_at: DateTimeWithTimeZone,
        pub updated_at: DateTimeWithTimeZone,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod issue {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "issue")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub workspace_id: Uuid,
        pub project_id: Uuid,
        pub milestone_id: Option<Uuid>,
        pub title: String,
        pub description: Option<String>,
        pub state_category: String,
        pub priority: String,
        pub archived_at: Option<DateTimeWithTimeZone>,
        pub row_version: i64,
        pub created_at: DateTimeWithTimeZone,
        pub updated_at: DateTimeWithTimeZone,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod activity {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "activity")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub workspace_id: Uuid,
        pub project_id: Uuid,
        pub issue_id: Option<Uuid>,
        pub command_id: Uuid,
        pub actor_id: Uuid,
        pub event_type: String,
        pub event_payload: Json,
        pub created_at: DateTimeWithTimeZone,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod outbox {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "outbox")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub workspace_id: Uuid,
        pub event_id: Uuid,
        pub event_type: String,
        pub aggregate_type: String,
        pub aggregate_id: Uuid,
        pub occurred_at: DateTimeWithTimeZone,
        pub correlation_id: Uuid,
        pub payload: Json,
        pub attempts: i32,
        pub available_at: DateTimeWithTimeZone,
        pub status: String,
        pub leased_at: Option<DateTimeWithTimeZone>,
        pub leased_until: Option<DateTimeWithTimeZone>,
        pub lease_token: Option<Uuid>,
        pub lease_owner: Option<Uuid>,
        pub published_at: Option<DateTimeWithTimeZone>,
        pub last_error: Option<String>,
        pub created_at: DateTimeWithTimeZone,
        pub updated_at: DateTimeWithTimeZone,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod idempotency_record {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "idempotency_record")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub workspace_id: Uuid,
        #[sea_orm(primary_key, auto_increment = false)]
        pub command_name: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub idempotency_key: String,
        pub request_fingerprint: String,
        pub response_payload: Json,
        pub expires_at: DateTimeWithTimeZone,
        pub created_at: DateTimeWithTimeZone,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}
