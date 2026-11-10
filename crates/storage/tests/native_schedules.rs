use cowork_contracts::*;
use cowork_domain::Error;
use cowork_storage::{Store, WriteSchedule};
use serde_json::json;
async fn fixture(store: &Store) -> (Project, Order) {
    let p = store
        .create_project(CreateProject {
            name: "Schedule project".into(),
        })
        .await
        .unwrap();
    let o = store
        .create_order(CreateOrder {
            project_id: p.id.clone(),
            title: "Scheduled chat".into(),
        })
        .await
        .unwrap();
    (p, o)
}
fn write(order: &Order) -> WriteSchedule {
    serde_json::from_value(json!({"id":null,"expected_revision":0,"name":"Morning review","timezone":"Europe/Berlin","recurrence":{"kind":"daily","hour":9,"minute":0},"template":{"order_id":order.id,"prompt":"Inspect current evidence","provider_id":"fixture","provider_revision":1},"paused":false})).unwrap()
}
#[tokio::test]
async fn lost_ack_create_replays_exact_receipt_even_after_a_later_edit() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (p, o) = fixture(&store).await;
    let original = write(&o);
    let first = store
        .productivity_write_schedule_idempotent(&p.id, original.clone(), "original-create")
        .await
        .unwrap();
    let mut edit = original.clone();
    edit.id = Some(first.id.clone());
    edit.expected_revision = 1;
    edit.name = "Revised schedule".into();
    let updated = store
        .productivity_write_schedule_idempotent(&p.id, edit, "edit")
        .await
        .unwrap();
    assert_eq!(updated.revision, 2);
    let replay = store
        .productivity_write_schedule_idempotent(&p.id, original.clone(), "original-create")
        .await
        .unwrap();
    assert_eq!(replay.id, first.id);
    assert_eq!(replay.revision, 1);
    assert_eq!(replay.name, "Morning review");
    assert_eq!(store.productivity_schedules(&p.id).await.unwrap().len(), 1);
    assert_eq!(
        store
            .native_schedule_original(&p.id, &original, "original-create")
            .await
            .unwrap()
            .unwrap()
            .revision,
        1
    );
    let mut changed = original;
    changed.name = "Different original".into();
    assert!(matches!(
        store
            .productivity_write_schedule_idempotent(&p.id, changed, "original-create")
            .await,
        Err(Error::Conflict)
    ));
}
#[tokio::test]
async fn native_history_retains_original_occurrence_without_replaying_or_mutating_it() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (p, o) = fixture(&store).await;
    let first = store
        .productivity_write_schedule(&p.id, write(&o))
        .await
        .unwrap();
    let claimed = store
        .productivity_claim_schedule(Some(&p.id), Some(&first.id), true, Some(first.revision))
        .await
        .unwrap()
        .unwrap();
    let page = store
        .native_schedule_history(&p.id, &first.id, None, 1)
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert!(page.next_cursor.is_some());
    let occurrence = page.items[0].occurrence.as_ref().unwrap();
    assert_eq!(occurrence.request.prompt, "Inspect current evidence");
    assert!(page.items[0].run.is_none());
    let older = store
        .native_schedule_history(&p.id, &first.id, page.next_cursor, 1)
        .await
        .unwrap();
    assert!(older.items[0].occurrence.is_none());
    assert!(older.next_cursor.is_none());
    let current = store.productivity_schedules(&p.id).await.unwrap().remove(0);
    assert_eq!(current.revision, claimed.revision);
    assert_eq!(current.active_occurrence.unwrap().id, occurrence.id);
    let spec:RunSpec=serde_json::from_value(json!({
        "project_id":p.id,"order_id":o.id,"prompt":occurrence.request.prompt,
        "provider":{"id":"fixture","name":"Fixture","kind":"test","endpoint":"","model":"test","revision":1,"context_tokens":32768,"credential_required":false,"credential_ready":true},
        "access_mode":"read_only","executor":"local-v2","policy_revision":1,"allowed_tools":[],"budgets":occurrence.request.budgets,
        "folders":[],"mcp_tools":[],"environment":null,"file_action":null,"workflow":null,"output":null,"model_binding":null,"agent_config":null
    })).unwrap();
    let run = store.create_run(&occurrence.request, spec).await.unwrap();
    store
        .productivity_schedule_dispatch_result(&p.id, &first.id, &occurrence.id, Some(&run))
        .await
        .unwrap();
    let admitted = store
        .native_schedule_history(&p.id, &first.id, None, 1)
        .await
        .unwrap();
    assert_eq!(admitted.items[0].run.as_ref().unwrap().id, run.id);
    assert_eq!(
        admitted.items[0].run.as_ref().unwrap().state,
        RunState::Queued
    );
    let (other, _) = fixture(&store).await;
    assert!(
        store
            .native_schedule_history(&other.id, &first.id, None, 50)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    assert!(matches!(
        store
            .native_schedule_history(&p.id, &first.id, Some(0), 50)
            .await,
        Err(Error::Validation(_))
    ));
}
#[tokio::test]
async fn deleting_native_schedule_preserves_revision_history() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (p, o) = fixture(&store).await;
    let first = store
        .productivity_write_schedule(&p.id, write(&o))
        .await
        .unwrap();
    store
        .productivity_delete_schedule(&p.id, &first.id, first.revision)
        .await
        .unwrap();
    assert!(
        store
            .productivity_schedules(&p.id)
            .await
            .unwrap()
            .is_empty()
    );
    let history = store
        .native_schedule_history(&p.id, &first.id, None, 50)
        .await
        .unwrap();
    assert_eq!(history.items.len(), 2);
    assert!(history.items[0].deleted);
    assert_eq!(history.items[1].schedule_revision, Some(1));
    assert_eq!(
        store.native_deleted_schedules(&p.id).await.unwrap()[0].name,
        first.name
    );
}

#[tokio::test]
async fn skill_versions_are_immutable_paginated_and_project_scoped() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (project, _) = fixture(&store).await;
    let mut input:WriteSkill=serde_json::from_value(json!({"id":null,"expected_revision":0,"name":"Reviewer","description":"","instructions":"Original evidence rule","trigger_pattern":"*review*","prompt_template":"Question: {{input}}","enabled":true})).unwrap();
    let first = store
        .productivity_write_skill(&project.id, input.clone())
        .await
        .unwrap();
    input.id = Some(first.id.clone());
    input.expected_revision = 1;
    input.instructions = "Improved evidence rule".into();
    store
        .productivity_write_skill(&project.id, input)
        .await
        .unwrap();
    let page = store
        .native_skill_history(&project.id, &first.id, None, 1)
        .await
        .unwrap();
    assert_eq!(
        page.items[0].skill.as_ref().unwrap().instructions,
        "Improved evidence rule"
    );
    assert!(page.next_cursor.is_some());
    let old = store
        .native_skill_history(&project.id, &first.id, page.next_cursor, 1)
        .await
        .unwrap();
    assert_eq!(
        old.items[0].skill.as_ref().unwrap().instructions,
        "Original evidence rule"
    );
    assert_eq!(
        old.items[0]
            .skill
            .as_ref()
            .unwrap()
            .trigger_pattern
            .as_deref(),
        Some("*review*")
    );
    assert!(old.next_cursor.is_none());
    let (other, _) = fixture(&store).await;
    assert!(
        store
            .native_skill_history(&other.id, &first.id, None, 50)
            .await
            .unwrap()
            .items
            .is_empty()
    );
}

#[test]
fn cron_uses_selected_timezone_and_never_dispatches_subminute() {
    use chrono::{TimeZone, Utc};
    use cowork_storage::{ScheduleRecurrence, productivity_next_run};
    let after = Utc
        .with_ymd_and_hms(2026, 10, 19, 0, 0, 0)
        .unwrap()
        .timestamp_millis();
    let rule = ScheduleRecurrence::Cron {
        expression: "0 9 * * MON-FRI".into(),
    };
    let next = productivity_next_run(&rule, "Asia/Singapore", after)
        .unwrap()
        .unwrap();
    assert_eq!(
        next,
        Utc.with_ymd_and_hms(2026, 10, 19, 1, 0, 0)
            .unwrap()
            .timestamp_millis()
    );
    assert_eq!(
        productivity_next_run(
            &ScheduleRecurrence::Cron {
                expression: "0 9 * * 1-5".into()
            },
            "Asia/Singapore",
            after
        )
        .unwrap(),
        Some(next)
    );
    let spring = Utc
        .with_ymd_and_hms(2026, 3, 28, 23, 0, 0)
        .unwrap()
        .timestamp_millis();
    assert_eq!(
        productivity_next_run(
            &ScheduleRecurrence::Cron {
                expression: "30 2 * * *".into()
            },
            "Europe/Berlin",
            spring
        )
        .unwrap(),
        Some(
            Utc.with_ymd_and_hms(2026, 3, 30, 0, 30, 0)
                .unwrap()
                .timestamp_millis()
        )
    );
    let friday = Utc
        .with_ymd_and_hms(2026, 10, 2, 0, 0, 0)
        .unwrap()
        .timestamp_millis();
    assert_eq!(
        productivity_next_run(
            &ScheduleRecurrence::Cron {
                expression: "0 9 */1 * MON".into()
            },
            "UTC",
            friday
        )
        .unwrap(),
        Some(
            Utc.with_ymd_and_hms(2026, 10, 5, 9, 0, 0)
                .unwrap()
                .timestamp_millis()
        )
    );
    // Both restricted day fields use the next Monday, rather than waiting for a Monday that is also the 1st.
    assert_eq!(
        productivity_next_run(
            &ScheduleRecurrence::Cron {
                expression: "0 0 1 * 1".into()
            },
            "UTC",
            friday
        )
        .unwrap(),
        Some(
            Utc.with_ymd_and_hms(2026, 10, 5, 0, 0, 0)
                .unwrap()
                .timestamp_millis()
        )
    );
    assert!(
        productivity_next_run(
            &ScheduleRecurrence::Cron {
                expression: "* * * * * *".into()
            },
            "UTC",
            after
        )
        .is_err()
    );
    assert!(
        productivity_next_run(
            &ScheduleRecurrence::Cron {
                expression: "60 25 * * *".into()
            },
            "UTC",
            after
        )
        .is_err()
    );
}
