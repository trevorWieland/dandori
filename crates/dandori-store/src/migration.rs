use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, Database};

use crate::StoreError;
use crate::migration_sql::{
    CREATE_ACTIVITY_OUTBOX_AND_IDEMPOTENCY_SQL, CREATE_ENUMS_AND_CORE_TABLES_SQL,
    CREATE_INDEXES_SQL, CREATE_WORKER_PARTITION_LEASE_SQL,
    DROP_ACTIVITY_OUTBOX_AND_IDEMPOTENCY_SQL, DROP_ENUMS_AND_CORE_TABLES_SQL, DROP_INDEXES_SQL,
    DROP_RLS_AND_POLICIES_SQL, DROP_WORKER_PARTITION_LEASE_SQL, ENABLE_RLS_AND_POLICIES_SQL,
};

pub async fn migrate_database(database_url: &str) -> Result<(), StoreError> {
    let db = Database::connect(database_url).await?;
    Migrator::up(&db, None).await?;
    Ok(())
}

pub(crate) struct Migrator;

#[sea_orm_migration::async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(CreateEnumsAndCoreTables),
            Box::new(CreateActivityOutboxAndIdempotency),
            Box::new(CreateIndexes),
            Box::new(EnableRlsAndPolicies),
            Box::new(CreateWorkerPartitionLease),
        ]
    }
}

struct CreateEnumsAndCoreTables;

struct CreateActivityOutboxAndIdempotency;

struct CreateIndexes;

struct EnableRlsAndPolicies;

struct CreateWorkerPartitionLease;

impl MigrationName for CreateEnumsAndCoreTables {
    fn name(&self) -> &str {
        "m20260415_000001_create_enums_and_core_tables"
    }
}

impl MigrationName for CreateActivityOutboxAndIdempotency {
    fn name(&self) -> &str {
        "m20260415_000002_create_activity_outbox_and_idempotency"
    }
}

impl MigrationName for CreateIndexes {
    fn name(&self) -> &str {
        "m20260415_000003_create_indexes"
    }
}

impl MigrationName for EnableRlsAndPolicies {
    fn name(&self) -> &str {
        "m20260415_000004_enable_rls_and_policies"
    }
}

impl MigrationName for CreateWorkerPartitionLease {
    fn name(&self) -> &str {
        "m20260416_000005_create_worker_partition_lease"
    }
}

#[sea_orm_migration::async_trait::async_trait]
impl MigrationTrait for CreateEnumsAndCoreTables {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(CREATE_ENUMS_AND_CORE_TABLES_SQL)
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(DROP_ENUMS_AND_CORE_TABLES_SQL)
            .await?;
        Ok(())
    }
}

#[sea_orm_migration::async_trait::async_trait]
impl MigrationTrait for CreateActivityOutboxAndIdempotency {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(CREATE_ACTIVITY_OUTBOX_AND_IDEMPOTENCY_SQL)
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(DROP_ACTIVITY_OUTBOX_AND_IDEMPOTENCY_SQL)
            .await?;
        Ok(())
    }
}

#[sea_orm_migration::async_trait::async_trait]
impl MigrationTrait for CreateIndexes {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(CREATE_INDEXES_SQL)
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(DROP_INDEXES_SQL)
            .await?;
        Ok(())
    }
}

#[sea_orm_migration::async_trait::async_trait]
impl MigrationTrait for EnableRlsAndPolicies {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(ENABLE_RLS_AND_POLICIES_SQL)
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(DROP_RLS_AND_POLICIES_SQL)
            .await?;
        Ok(())
    }
}

#[sea_orm_migration::async_trait::async_trait]
impl MigrationTrait for CreateWorkerPartitionLease {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(CREATE_WORKER_PARTITION_LEASE_SQL)
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(DROP_WORKER_PARTITION_LEASE_SQL)
            .await?;
        Ok(())
    }
}
