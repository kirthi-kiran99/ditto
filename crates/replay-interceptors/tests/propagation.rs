use replay_core::{
    current_ctx, next_interaction_slot, spawn_with_ctx, with_recording, CallType, ReplayMode,
};

#[tokio::test]
async fn context_propagates_through_spawn_with_ctx() {
    with_recording(ReplayMode::Record, async {
        let parent_id = current_ctx().unwrap().record_id;

        let handle = spawn_with_ctx(async {
            current_ctx().map(|c| c.record_id)
        });

        let spawned_id = handle.await.unwrap();
        assert_eq!(Some(parent_id), spawned_id);
    })
    .await;
}

#[tokio::test]
async fn three_nested_calls_share_record_id_and_increment_sequence() {
    with_recording(ReplayMode::Record, async {
        let record_id = current_ctx().unwrap().record_id;

        let s1 = next_interaction_slot(CallType::Http, "GET /a".into()).unwrap();
        let s2 = next_interaction_slot(CallType::Postgres, "SELECT ?".into()).unwrap();
        let s3 = next_interaction_slot(CallType::Redis, "cache:{id}".into()).unwrap();

        assert_eq!(s1.record_id, record_id);
        assert_eq!(s2.record_id, record_id);
        assert_eq!(s3.record_id, record_id);

        assert_eq!(s1.sequence, 0);
        assert_eq!(s2.sequence, 1);
        assert_eq!(s3.sequence, 2);
    })
    .await;
}

#[tokio::test]
async fn spawned_tasks_inherit_correct_mode() {
    with_recording(ReplayMode::Replay, async {
        let handle = spawn_with_ctx(async {
            current_ctx().map(|c| c.mode.clone())
        });
        let mode = handle.await.unwrap();
        assert!(matches!(mode, Some(ReplayMode::Replay)));
    })
    .await;
}

#[tokio::test]
async fn no_context_outside_scope_returns_none() {
    // Deliberately outside any with_recording scope
    let ctx = current_ctx();
    assert!(ctx.is_none());
}
