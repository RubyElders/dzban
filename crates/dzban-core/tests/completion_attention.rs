use dzban_core::attention::{
    AttentionInput, AttentionPolicy, CompletionIdentity, CompletionOutcome, ViewedOutput,
};
use uuid::Uuid;

fn identity(tab: Uuid, document: Uuid, generation: u64) -> CompletionIdentity {
    CompletionIdentity {
        output: ViewedOutput {
            tab,
            document,
            generation,
        },
        session: tab,
        query: Uuid::new_v4(),
    }
}
fn add(policy: &mut AttentionPolicy, generation: u64) -> CompletionIdentity {
    let result = identity(Uuid::new_v4(), Uuid::new_v4(), generation);
    policy.register(result.output.tab, result.output.document);
    policy.started(result);
    result
}

#[test]
fn visible_completions_mark_only_background_tabs_without_requesting_attention() {
    let mut policy = AttentionPolicy::default();
    let selected = add(&mut policy, 1);
    let background = add(&mut policy, 1);
    policy.input(AttentionInput::Viewed {
        output: Some(selected.output),
    });
    policy.completed(selected, CompletionOutcome::Success);
    policy.completed(background, CompletionOutcome::Failure);
    let snapshot = policy.snapshot();
    assert_eq!(snapshot.request_revision, 0);
    assert!(snapshot.cancel_attention);
    let entry = snapshot
        .tabs
        .iter()
        .find(|entry| entry.tab == selected.output.tab)
        .unwrap();
    assert!(entry.unseen.is_none());
    assert_eq!(entry.completion_revision, 0);
    let entry = snapshot
        .tabs
        .iter()
        .find(|entry| entry.tab == background.output.tab)
        .unwrap();
    assert_eq!(
        entry.unseen.as_ref().unwrap().outcome,
        CompletionOutcome::Failure
    );
    assert_eq!(entry.completion_revision, 1);
}

#[test]
fn away_completions_are_grouped_and_visibility_acknowledges_only_the_viewed_output() {
    let mut policy = AttentionPolicy::default();
    let first = add(&mut policy, 1);
    let second = add(&mut policy, 1);
    policy.input(AttentionInput::Visibility {
        visible: false,
        viewed: Some(first.output),
    });
    policy.completed(first, CompletionOutcome::Success);
    policy.completed(second, CompletionOutcome::Failure);
    policy.completed(first, CompletionOutcome::Success);
    assert_eq!(policy.snapshot().request_revision, 1);
    assert_eq!(
        policy
            .snapshot()
            .tabs
            .iter()
            .filter(|entry| entry.unseen.is_some())
            .count(),
        2
    );
    policy.input(AttentionInput::Viewed {
        output: Some(second.output),
    });
    assert_eq!(
        policy
            .snapshot()
            .tabs
            .iter()
            .filter(|entry| entry.unseen.is_some())
            .count(),
        2
    );
    policy.input(AttentionInput::Visibility {
        visible: true,
        viewed: Some(second.output),
    });
    assert_eq!(
        policy
            .snapshot()
            .tabs
            .iter()
            .filter(|entry| entry.unseen.is_some())
            .count(),
        1
    );
    policy.input(AttentionInput::Viewed {
        output: Some(first.output),
    });
    assert!(
        policy
            .snapshot()
            .tabs
            .iter()
            .all(|entry| entry.unseen.is_none())
    );
    policy.input(AttentionInput::Visibility {
        visible: false,
        viewed: None,
    });
    let next = identity(first.output.tab, first.output.document, 2);
    policy.started(next);
    policy.completed(next, CompletionOutcome::Success);
    assert_eq!(policy.snapshot().request_revision, 2);
}

#[test]
fn a_stale_view_cannot_clear_newer_output_and_replayed_completion_cannot_reopen_acknowledged_output()
 {
    let mut policy = AttentionPolicy::default();
    let first = add(&mut policy, 1);
    policy.input(AttentionInput::Visibility {
        visible: false,
        viewed: None,
    });
    policy.completed(first, CompletionOutcome::Success);
    let next = identity(first.output.tab, first.output.document, u64::MAX);
    policy.started(next);
    policy.completed(next, CompletionOutcome::Failure);
    policy.input(AttentionInput::Visibility {
        visible: true,
        viewed: Some(first.output),
    });
    let snapshot = policy.snapshot();
    assert_eq!(snapshot.tabs[0].unseen.as_ref().unwrap().identity, next);
    assert_eq!(snapshot.tabs[0].completion_revision, 2);
    policy.input(AttentionInput::Viewed {
        output: Some(next.output),
    });
    assert!(policy.snapshot().tabs[0].unseen.is_none());
    policy.input(AttentionInput::Visibility {
        visible: false,
        viewed: None,
    });
    policy.completed(next, CompletionOutcome::Failure);
    assert!(policy.snapshot().tabs[0].unseen.is_none());
    assert_eq!(policy.snapshot().request_revision, 1);
}

#[test]
fn retired_wrong_document_and_superseded_completions_are_ignored() {
    let mut policy = AttentionPolicy::default();
    let first = add(&mut policy, 1);
    let next = identity(first.output.tab, first.output.document, 2);
    policy.started(next);
    policy.completed(first, CompletionOutcome::Success);
    let wrong_document = identity(first.output.tab, Uuid::new_v4(), 3);
    policy.started(wrong_document);
    policy.completed(wrong_document, CompletionOutcome::Failure);
    assert!(policy.snapshot().tabs[0].unseen.is_none());
    policy.retire(first.output.tab);
    policy.completed(next, CompletionOutcome::Success);
    assert!(policy.snapshot().tabs.is_empty());
}

#[test]
fn cancellation_close_and_shutdown_suppress_notifications_without_replaying_them_on_retry() {
    let mut policy = AttentionPolicy::default();
    let first = add(&mut policy, 1);
    policy.input(AttentionInput::Visibility {
        visible: false,
        viewed: None,
    });
    policy.completed(first, CompletionOutcome::Canceled);
    assert_eq!(policy.snapshot().tabs[0].completion_revision, 0);
    let next = identity(first.output.tab, first.output.document, 2);
    policy.started(next);
    policy.closing(first.output.tab, true);
    policy.completed(next, CompletionOutcome::Success);
    policy.closing(first.output.tab, false);
    policy.completed(next, CompletionOutcome::Success);
    assert_eq!(policy.snapshot().request_revision, 0);
    let next = identity(first.output.tab, first.output.document, 3);
    policy.started(next);
    policy.suspend(true);
    policy.completed(next, CompletionOutcome::Failure);
    policy.suspend(false);
    policy.completed(next, CompletionOutcome::Failure);
    assert_eq!(policy.snapshot().request_revision, 0);
    let next = identity(first.output.tab, first.output.document, 4);
    policy.started(next);
    policy.completed(next, CompletionOutcome::Success);
    assert_eq!(policy.snapshot().request_revision, 1);
    policy.clear();
    assert!(policy.snapshot().tabs.is_empty());
    assert!(policy.snapshot().cancel_attention);
}

#[test]
fn chooser_and_host_lifecycle_reports_preserve_grouping_and_suppression() {
    let mut policy = AttentionPolicy::default();
    let first = add(&mut policy, 1);
    policy.input(AttentionInput::Viewed {
        output: Some(first.output),
    });
    policy.input(AttentionInput::Viewed { output: None });
    policy.completed(first, CompletionOutcome::Success);
    assert!(policy.snapshot().tabs[0].unseen.is_some());
    policy.input(AttentionInput::Visibility {
        visible: false,
        viewed: None,
    });
    policy.input(AttentionInput::Lifecycle {
        restoring: true,
        quitting: false,
    });
    assert!(policy.snapshot().cancel_attention);
    let next = identity(first.output.tab, first.output.document, 2);
    policy.started(next);
    policy.completed(next, CompletionOutcome::Success);
    assert_eq!(policy.snapshot().request_revision, 0);
    policy.input(AttentionInput::Lifecycle {
        restoring: false,
        quitting: true,
    });
    assert!(policy.snapshot().cancel_attention);
    let next = identity(first.output.tab, first.output.document, 3);
    policy.started(next);
    policy.completed(next, CompletionOutcome::Failure);
    assert_eq!(policy.snapshot().request_revision, 0);
    policy.input(AttentionInput::Lifecycle {
        restoring: false,
        quitting: false,
    });
    let next = identity(first.output.tab, first.output.document, 4);
    policy.started(next);
    policy.completed(next, CompletionOutcome::Success);
    assert_eq!(policy.snapshot().request_revision, 1);
}

#[test]
fn viewing_replacement_output_acknowledges_an_older_unseen_completion() {
    let mut policy = AttentionPolicy::default();
    let first = add(&mut policy, 1);
    policy.input(AttentionInput::Visibility {
        visible: false,
        viewed: None,
    });
    policy.completed(first, CompletionOutcome::Success);
    policy.input(AttentionInput::Visibility {
        visible: true,
        viewed: Some(ViewedOutput {
            generation: 2,
            ..first.output
        }),
    });
    assert!(policy.snapshot().tabs[0].unseen.is_none());
    assert_eq!(policy.snapshot().tabs[0].completion_revision, 1);
}
