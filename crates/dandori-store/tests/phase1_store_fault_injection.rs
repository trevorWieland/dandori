mod support;

use sqlx::{Row, query};
use uuid::Uuid;

use support::{auth_context, make_command, make_event, setup_db};

#[tokio::test]
async fn fault_injection_after_issue_insert_rolls_back_issue_activity_outbox_and_idempotency() {
    let db = setup_db().await;

    query(
        "CREATE OR REPLACE FUNCTION test_fail_outbox_insert() RETURNS trigger AS $$
            BEGIN
                RAISE EXCEPTION 'forced outbox failure';
            END;
        $$ LANGUAGE plpgsql",
    )
    .execute(&db.admin_pool)
    .await
    .expect("create test function");

    query(
        "CREATE TRIGGER test_fail_outbox_insert_trigger
         BEFORE INSERT ON outbox
         FOR EACH ROW
         EXECUTE FUNCTION test_fail_outbox_insert()",
    )
    .execute(&db.admin_pool)
    .await
    .expect("create test trigger");

    let command = make_command(
        db.workspace_a,
        db.project_a,
        Uuid::now_v7(),
        Uuid::now_v7(),
        "idem-fault-inject",
    );
    let event = make_event(&command);
    let auth = auth_context(db.workspace_a, command.actor_id);

    let _error = db
        .app_store
        .create_issue_transactional(&auth, &command, &event)
        .await
        .expect_err("fault injection should fail write path");

    let issue_count: i64 = query("SELECT COUNT(*) AS count FROM issue WHERE id = $1")
        .bind(command.issue_id.0)
        .fetch_one(&db.admin_pool)
        .await
        .expect("issue count")
        .get("count");

    let activity_count: i64 = query("SELECT COUNT(*) AS count FROM activity WHERE command_id = $1")
        .bind(command.command_id.0)
        .fetch_one(&db.admin_pool)
        .await
        .expect("activity count")
        .get("count");

    let outbox_count: i64 = query("SELECT COUNT(*) AS count FROM outbox WHERE correlation_id = $1")
        .bind(command.command_id.0)
        .fetch_one(&db.admin_pool)
        .await
        .expect("outbox count")
        .get("count");

    let idempotency_count: i64 = query(
        "SELECT COUNT(*) AS count
         FROM idempotency_record
         WHERE workspace_id = $1 AND command_name = $2 AND idempotency_key = $3",
    )
    .bind(command.workspace_id.0)
    .bind("issue.create.v1")
    .bind(command.idempotency_key.as_str())
    .fetch_one(&db.admin_pool)
    .await
    .expect("idempotency count")
    .get("count");

    assert_eq!(issue_count, 0);
    assert_eq!(activity_count, 0);
    assert_eq!(outbox_count, 0);
    assert_eq!(idempotency_count, 0);
}
