//! Correctness proofs for the dynamic partition leasing model.
//!
//! Workers lease workspaces from `worker_partition_lease` using an atomic
//! `INSERT … ON CONFLICT DO UPDATE WHERE leased_until <= now` pattern. These
//! tests exercise:
//!
//! 1. Two owners concurrently leasing → disjoint sets of partitions.
//! 2. Lease renewal by the current owner.
//! 3. Expired leases are taken over by a new owner.

use std::collections::HashSet;

use chrono::{Duration, Utc};
use dandori_test_support::setup_database;
use uuid::Uuid;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_concurrent_owners_acquire_disjoint_partitions() {
    let db = setup_database().await;
    let store = db.app_store.clone();

    let owner_a = Uuid::now_v7();
    let owner_b = Uuid::now_v7();
    let now = Utc::now();
    let until = now + Duration::seconds(30);

    let store_a = store.clone();
    let store_b = store.clone();
    let acquire_a =
        tokio::spawn(async move { store_a.acquire_partitions(owner_a, now, until, 10).await });
    let acquire_b =
        tokio::spawn(async move { store_b.acquire_partitions(owner_b, now, until, 10).await });

    let claims_a: HashSet<Uuid> = acquire_a
        .await
        .expect("join a")
        .expect("acquire a")
        .into_iter()
        .collect();
    let claims_b: HashSet<Uuid> = acquire_b
        .await
        .expect("join b")
        .expect("acquire b")
        .into_iter()
        .collect();

    assert!(
        claims_a.is_disjoint(&claims_b),
        "owners must never share a partition: a={claims_a:?} b={claims_b:?}"
    );

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM worker_partition_lease")
        .fetch_one(&db.admin_pool)
        .await
        .expect("count leases");
    let combined_len = i64::try_from(claims_a.len() + claims_b.len()).expect("size");
    assert_eq!(total, combined_len);
}

#[tokio::test]
async fn owner_can_renew_its_own_lease() {
    let db = setup_database().await;
    let store = db.app_store.clone();

    let owner = Uuid::now_v7();
    let now = Utc::now();
    let until = now + Duration::seconds(10);
    let claimed = store
        .acquire_partitions(owner, now, until, 10)
        .await
        .expect("acquire");

    let new_until = now + Duration::seconds(60);
    let renewed = store
        .renew_partitions(owner, &claimed, now, new_until)
        .await
        .expect("renew");
    assert_eq!(renewed.len(), claimed.len());
}

#[tokio::test]
async fn expired_lease_is_taken_over_by_a_new_owner() {
    let db = setup_database().await;
    let store = db.app_store.clone();

    let first_owner = Uuid::now_v7();
    let second_owner = Uuid::now_v7();

    let original_now = Utc::now() - Duration::seconds(120);
    let original_until = original_now + Duration::seconds(30);
    let first_claim = store
        .acquire_partitions(first_owner, original_now, original_until, 10)
        .await
        .expect("first acquire");
    assert!(!first_claim.is_empty());

    let takeover_now = Utc::now();
    let takeover_until = takeover_now + Duration::seconds(30);
    let second_claim = store
        .acquire_partitions(second_owner, takeover_now, takeover_until, 10)
        .await
        .expect("takeover acquire");

    for workspace_id in first_claim {
        assert!(
            second_claim.contains(&workspace_id),
            "expired lease must be re-acquired by new owner: {workspace_id}"
        );
    }
}

#[tokio::test]
async fn releasing_partitions_is_scoped_to_current_owner() {
    let db = setup_database().await;
    let store = db.app_store.clone();

    let owner_a = Uuid::now_v7();
    let owner_b = Uuid::now_v7();
    let now = Utc::now();
    let until = now + Duration::seconds(60);

    let claim_a = store
        .acquire_partitions(owner_a, now, until, 1)
        .await
        .expect("acquire a");
    let claim_b = store
        .acquire_partitions(owner_b, now, until, 1)
        .await
        .expect("acquire b");

    // Owner B trying to release owner A's partitions is a no-op.
    let b_attempt_on_a = store
        .release_partitions(owner_b, &claim_a)
        .await
        .expect("release b over a");
    assert_eq!(b_attempt_on_a, 0);

    let a_release_a = store
        .release_partitions(owner_a, &claim_a)
        .await
        .expect("release a over a");
    assert_eq!(a_release_a as usize, claim_a.len());

    let b_release_b = store
        .release_partitions(owner_b, &claim_b)
        .await
        .expect("release b over b");
    assert_eq!(b_release_b as usize, claim_b.len());
}
