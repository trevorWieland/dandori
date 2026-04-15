use std::time::Instant;

use dandori_app_services::{IssueAppService, OutboxWorkerConfig, OutboxWorkerService};
use dandori_contract::{CreateIssueRequest, IssuePriorityDto};
use dandori_domain::AuthContext;
use dandori_store::{PgStore, migrate_database};
use sqlx::PgPool;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use uuid::Uuid;

struct TestService {
    _container: testcontainers::ContainerAsync<Postgres>,
    auth: AuthContext,
    project_id: Uuid,
    store: PgStore,
    service: IssueAppService,
}

async fn setup() -> TestService {
    let container = Postgres::default().start().await.expect("start postgres");
    let host = container.get_host().await.expect("host");
    let port = container.get_host_port_ipv4(5432).await.expect("port");

    let admin_url = format!("postgres://postgres:postgres@{host}:{port}/postgres");
    migrate_database(&admin_url).await.expect("migrate");

    let admin_pool = PgPool::connect(&admin_url).await.expect("connect admin");

    sqlx::query(
        "DO $$
            BEGIN
                IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'dandori_app') THEN
                    CREATE ROLE dandori_app
                        LOGIN
                        PASSWORD 'dandori_app'
                        NOSUPERUSER
                        NOCREATEDB
                        NOCREATEROLE
                        NOBYPASSRLS;
                END IF;
            END
        $$;",
    )
    .execute(&admin_pool)
    .await
    .expect("create app role");

    sqlx::query("GRANT USAGE ON SCHEMA public TO dandori_app")
        .execute(&admin_pool)
        .await
        .expect("grant schema usage");

    sqlx::query(
        "GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO dandori_app",
    )
    .execute(&admin_pool)
    .await
    .expect("grant table perms");

    let workspace_id = Uuid::now_v7();
    let workflow_id = Uuid::now_v7();
    let project_id = Uuid::now_v7();

    sqlx::query("INSERT INTO workspace (id, name) VALUES ($1, 'ws-a')")
        .bind(workspace_id)
        .execute(&admin_pool)
        .await
        .expect("seed workspace");

    sqlx::query(
        "INSERT INTO workflow_version (id, workspace_id, name, version, checksum, states, transitions)
         VALUES ($1, $2, 'default', 1, 'sha256:a', '[]'::jsonb, '[]'::jsonb)",
    )
    .bind(workflow_id)
    .bind(workspace_id)
    .execute(&admin_pool)
    .await
    .expect("seed workflow");

    sqlx::query(
        "INSERT INTO project (id, workspace_id, name, workflow_version_id)
         VALUES ($1, $2, 'project-a', $3)",
    )
    .bind(project_id)
    .bind(workspace_id)
    .bind(workflow_id)
    .execute(&admin_pool)
    .await
    .expect("seed project");

    let app_url = format!("postgres://dandori_app:dandori_app@{host}:{port}/postgres");
    let store = PgStore::connect(&app_url).await.expect("connect app store");
    let service = IssueAppService::new(store.clone());

    TestService {
        _container: container,
        auth: AuthContext {
            workspace_id: workspace_id.into(),
            actor_id: Uuid::now_v7(),
        },
        project_id,
        store,
        service,
    }
}

#[tokio::test]
async fn app_service_create_issue_meets_baseline_throughput_budget() {
    let test = setup().await;
    let iterations: u32 = 24;
    let started = Instant::now();

    for n in 0..iterations {
        let request = CreateIssueRequest {
            idempotency_key: format!("perf-app-create-{n}"),
            project_id: test.project_id,
            milestone_id: None,
            title: format!("perf issue {n}"),
            description: Some("perf".to_owned()),
            priority: IssuePriorityDto::Medium,
        };
        test.service
            .create_issue(&test.auth, request)
            .await
            .expect("create issue");
    }

    let elapsed = started.elapsed();
    let per_op_ms = elapsed.as_secs_f64() * 1_000.0 / f64::from(iterations);
    assert!(
        per_op_ms < 300.0,
        "app-service create_issue throughput regression: {per_op_ms:.2}ms/op"
    );
}

#[tokio::test]
async fn worker_run_once_meets_baseline_throughput_budget() {
    let test = setup().await;
    let iterations: u32 = 36;

    for n in 0..iterations {
        let request = CreateIssueRequest {
            idempotency_key: format!("perf-worker-{n}"),
            project_id: test.project_id,
            milestone_id: None,
            title: format!("perf worker issue {n}"),
            description: Some("perf".to_owned()),
            priority: IssuePriorityDto::Medium,
        };
        test.service
            .create_issue(&test.auth, request)
            .await
            .expect("create issue");
    }

    let worker = OutboxWorkerService::with_default_publisher(
        test.store,
        OutboxWorkerConfig {
            workspace_id: test.auth.workspace_id.0,
            actor_id: test.auth.actor_id,
            batch_size: 64,
            lease_seconds: 30,
            max_attempts: 3,
            retry_backoff_seconds: 0,
            delivered_retention_hours: 1_000,
            dead_letter_retention_hours: 1_000,
            idempotency_retention_hours: 1_000,
        },
    );

    let started = Instant::now();
    let report = worker.run_once().await.expect("worker run");
    let elapsed = started.elapsed();

    assert_eq!(report.leased, usize::try_from(iterations).expect("usize"));
    assert_eq!(
        report.delivered,
        usize::try_from(iterations).expect("usize")
    );
    let per_op_ms = elapsed.as_secs_f64() * 1_000.0 / f64::from(iterations);
    assert!(
        per_op_ms < 200.0,
        "worker delivery throughput regression: {per_op_ms:.2}ms/op"
    );
}
