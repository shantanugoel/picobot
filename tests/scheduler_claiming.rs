use std::sync::Arc;

use picobot::scheduler::job::{CreateJobRequest, Principal, PrincipalType, ScheduleType};
use picobot::scheduler::store::ScheduleStore;
use picobot::session::db::SqliteStore;

fn temp_store() -> (ScheduleStore, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("picobot-scheduler-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("schedules.db");
    let store = ScheduleStore::new(SqliteStore::new(path.to_string_lossy().to_string()));
    let _ = store.store().touch();
    (store, dir)
}

fn create_job_request(user_id: &str, schedule_type: ScheduleType, expr: &str) -> CreateJobRequest {
    CreateJobRequest {
        name: "job".to_string(),
        schedule_type,
        schedule_expr: expr.to_string(),
        task_prompt: "ping".to_string(),
        session_id: None,
        user_id: user_id.to_string(),
        channel_id: None,
        capabilities: picobot::kernel::permissions::CapabilitySet::empty(),
        creator: Principal {
            principal_type: PrincipalType::User,
            id: user_id.to_string(),
        },
        enabled: true,
        max_executions: None,
        created_by_system: false,
        metadata: None,
    }
}

#[test]
fn expired_claim_is_reclaimed() {
    let (store, dir) = temp_store();
    let now = chrono::Utc::now();
    let job = store
        .create_job(create_job_request("user", ScheduleType::Once, "now"), now)
        .unwrap();
    let claim = store.claim_due_jobs(now, 1, "claim-1", 1).unwrap();
    assert_eq!(claim.len(), 1);
    assert_eq!(claim[0].id, job.id);

    let later = now + chrono::Duration::seconds(2);
    let reclaimed = store.claim_due_jobs(later, 1, "claim-2", 1).unwrap();
    assert_eq!(reclaimed.len(), 1);
    assert_eq!(reclaimed[0].id, job.id);

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn no_duplicate_execution_on_restart() {
    let (store, dir) = temp_store();
    let now = chrono::Utc::now();
    let job = store
        .create_job(create_job_request("user", ScheduleType::Once, "now"), now)
        .unwrap();
    let claim = store.claim_due_jobs(now, 1, "claim-1", 30).unwrap();
    assert_eq!(claim.len(), 1);
    assert_eq!(claim[0].id, job.id);

    let path = store.store().path().to_string();
    let restarted = ScheduleStore::new(SqliteStore::new(path));
    let later = now + chrono::Duration::seconds(1);
    let second = restarted
        .claim_due_jobs(later, 1, "claim-2", 30)
        .unwrap();
    assert!(second.is_empty());

    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn concurrent_workers_claim_disjoint_jobs() {
    let (store, dir) = temp_store();
    let now = chrono::Utc::now();
    let job_one = store
        .create_job(create_job_request("user", ScheduleType::Once, "now"), now)
        .unwrap();
    let job_two = store
        .create_job(create_job_request("user", ScheduleType::Once, "now"), now)
        .unwrap();
    let store = Arc::new(store);

    let store_left = Arc::clone(&store);
    let store_right = Arc::clone(&store);
    let left = tokio::task::spawn_blocking(move || {
        store_left
            .claim_due_jobs(now, 1, "claim-left", 30)
            .unwrap()
    });
    let right = tokio::task::spawn_blocking(move || {
        store_right
            .claim_due_jobs(now, 1, "claim-right", 30)
            .unwrap()
    });
    let left = left.await.unwrap();
    let right = right.await.unwrap();

    assert_eq!(left.len(), 1);
    assert_eq!(right.len(), 1);
    let left_id = left[0].id.clone();
    let right_id = right[0].id.clone();
    assert_ne!(left_id, right_id);
    assert!(left_id == job_one.id || left_id == job_two.id);
    assert!(right_id == job_one.id || right_id == job_two.id);

    std::fs::remove_dir_all(dir).ok();
}
