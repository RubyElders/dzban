use dzban_core::application::{
    ApplicationError, ConnectionPhase, ConnectionRequestId, ConnectionSelection, ConnectionSnapshot,
};
use dzban_core::execution::SessionId;
use dzban_core::{ConnectionProfileId, DatabaseBackend};

fn snapshot(request: ConnectionRequestId, phase: ConnectionPhase) -> ConnectionSnapshot {
    ConnectionSnapshot {
        request,
        phase,
        profile: ConnectionProfileId("fixture".into()),
        backend: DatabaseBackend::Postgresql,
    }
}

#[test]
fn superseded_connection_results_cannot_replace_the_current_selection() {
    let first = ConnectionRequestId(uuid::Uuid::new_v4());
    let second = ConnectionRequestId(uuid::Uuid::new_v4());
    let id = SessionId(uuid::Uuid::new_v4());
    let mut selection = ConnectionSelection::default();
    selection.begin(first);
    assert!(selection.apply(&snapshot(first, ConnectionPhase::Connecting)));
    selection.begin(second);
    assert!(!selection.apply(&snapshot(first, ConnectionPhase::Connected(id))));
    assert_eq!(selection, ConnectionSelection::Pending(second));
    assert!(selection.apply(&snapshot(second, ConnectionPhase::Connected(id))));
    assert_eq!(selection, ConnectionSelection::Ready(id));
    assert!(!selection.apply(&snapshot(
        first,
        ConnectionPhase::Failed(ApplicationError::UnknownSession(id))
    )));
    assert_eq!(selection, ConnectionSelection::Ready(id));
}

#[test]
fn disconnect_and_terminal_failure_reject_later_completion() {
    let request = ConnectionRequestId(uuid::Uuid::new_v4());
    let id = SessionId(uuid::Uuid::new_v4());
    let mut selection = ConnectionSelection::default();
    selection.begin(request);
    selection.clear();
    assert!(!selection.apply(&snapshot(request, ConnectionPhase::Connected(id))));
    assert_eq!(selection, ConnectionSelection::Empty);
    selection.begin(request);
    let error = ApplicationError::UnknownSession(id);
    assert!(selection.apply(&snapshot(request, ConnectionPhase::Failed(error.clone()))));
    assert_eq!(selection, ConnectionSelection::Failed(error));
    assert!(!selection.apply(&snapshot(request, ConnectionPhase::Connected(id))));
}

#[test]
fn an_open_selection_requires_disconnect_before_a_new_connection() {
    let request = ConnectionRequestId(uuid::Uuid::new_v4());
    let replacement = ConnectionRequestId(uuid::Uuid::new_v4());
    let id = SessionId(uuid::Uuid::new_v4());
    let mut selection = ConnectionSelection::default();
    assert!(selection.accepts_connection_request());
    assert!(selection.begin(request));
    assert!(selection.accepts_connection_request());
    assert!(selection.apply(&snapshot(request, ConnectionPhase::Connected(id))));
    assert!(!selection.accepts_connection_request());
    assert!(!selection.begin(replacement));
    assert_eq!(selection, ConnectionSelection::Ready(id));
    selection.clear();
    assert!(selection.begin(replacement));
    assert_eq!(selection, ConnectionSelection::Pending(replacement));
}

#[test]
fn pending_cleanup_blocks_reconnect_and_only_its_result_can_complete_the_close() {
    let id = SessionId(uuid::Uuid::new_v4());
    let other = SessionId(uuid::Uuid::new_v4());
    let mut selection = ConnectionSelection::Ready(id);
    assert!(!selection.begin_close(other));
    assert!(selection.begin_close(id));
    assert!(!selection.can_forget());
    assert!(!selection.accepts_connection_request());
    assert!(!selection.begin(ConnectionRequestId(uuid::Uuid::new_v4())));
    assert!(!selection.finish_close(other, &Ok(())));
    assert_eq!(selection, ConnectionSelection::Closing(id));
    assert!(selection.finish_close(id, &Err(dzban_core::error::ExecutionError::Busy)));
    assert_eq!(selection, ConnectionSelection::Ready(id));
    selection.begin_close(id);
    assert!(selection.finish_close(id, &Ok(())));
    assert_eq!(selection, ConnectionSelection::Empty);
    assert!(selection.can_forget());
    assert!(!selection.finish_close(id, &Err(dzban_core::error::ExecutionError::Busy)));
    assert!(selection.accepts_connection_request());
}
