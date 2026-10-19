use super::*;
pub use dzban_core::interaction::InputEvent as Input;

impl State {
    pub(super) async fn observe_health(&self, stream: Arc<Stream>) {
        let mut snapshots = self.context.health.subscribe();
        let mut phase = self.context.subscribe_phase();
        loop {
            stream.publish(json!(snapshots.borrow_and_update().clone()));
            if *phase.borrow_and_update() == ApplicationPhase::Closed {
                return;
            }
            tokio::select! {
                _ = stream.closed() => return,
                changed = snapshots.changed() => if changed.is_err() { return; },
                changed = phase.changed() => if changed.is_err() { return; },
            }
        }
    }

    pub(super) async fn observe_interaction(&self, session: Uuid, stream: Arc<Stream>) {
        let Some(tab) = self.context.tabs.session(SessionId(session)) else {
            stream.publish(failure(
                "selection_superseded",
                "The workspace is no longer selected",
            ));
            return;
        };
        let mut snapshots = match self.context.tabs.subscribe_interaction(tab.id) {
            Ok(snapshots) => snapshots,
            Err(error) => {
                stream.publish(application_error("interaction", error));
                return;
            }
        };
        let mut published = 0;
        loop {
            if let Some(snapshot) = snapshots.borrow_and_update().clone()
                && snapshot.revision > published
            {
                published = snapshot.revision;
                let retired = snapshot.retired;
                stream.publish(json!(snapshot));
                if retired {
                    return;
                }
            }
            tokio::select! {
                _ = stream.closed() => break,
                changed = snapshots.changed() => if changed.is_err() { break; },
            }
        }
    }

    pub(super) fn input(
        &self,
        session: Uuid,
        document: Uuid,
        revision: u64,
        input: Input,
    ) -> Result<Value, Value> {
        let tab = self
            .context
            .tabs
            .session(SessionId(session))
            .ok_or_else(|| {
                failure(
                    "selection_superseded",
                    "The workspace is no longer selected",
                )
            })?;
        self.context
            .tabs
            .input(tab.id, DocumentId(document), revision, input)
            .map(|result| json!(result))
            .map_err(|error| application_error("input", error))
    }

    pub(super) async fn await_idle(&self, session: Uuid) -> Result<Value, Value> {
        let engine = self
            .context
            .session(SessionId(session))
            .await
            .map_err(|error| application_error("session", error))?;
        dzban_core::commands::await_query_admission(&engine)
            .await
            .map(|idle| json!({"idle":idle}))
            .map_err(execution_error)
    }

    pub(super) async fn probe_session(
        &self,
        session: Uuid,
        include_version: bool,
    ) -> Result<Value, Value> {
        let engine = self
            .context
            .tabs
            .session(SessionId(session))
            .and_then(|tab| tab.engine)
            .ok_or_else(|| failure("unknown_session", "Session is not open"))?;
        self.context
            .health
            .start_session(engine, include_version)
            .wait()
            .await
            .map(|result| json!(result))
            .map_err(|error| application_error("probe", error))
    }

    pub(super) async fn probe_profile(&self, profile: String) -> Result<Value, Value> {
        let config = self
            .config
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| failure("configuration", "Load profiles before probing"))?;
        let config = config
            .resolve_profile(&ConnectionProfileId(profile), None)
            .map_err(|error| failure("profile", error))?;
        self.context
            .health
            .start_profile(self.context.clone(), config)
            .wait()
            .await
            .map(|result| json!(result))
            .map_err(|error| application_error("probe_cleanup", error))
    }
}
