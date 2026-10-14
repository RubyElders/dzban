use std::sync::Arc;

use crate::documents::{DocumentExecution, DocumentId, DocumentLifecycle, QueryDocument};
use crate::error::ExecutionError;
use crate::execution::{QueryEngine, QueryId, QueryPhase, SessionId, StreamedQueryCompletion};
use crate::history::QueryHistoryStore;
use crate::result_store::ResultLimits;

pub struct DocumentQueryHandle {
    pub document: DocumentId,
    pub query: QueryId,
    pub session: SessionId,
    pub generation: u64,
    task: tokio::task::JoinHandle<Result<StreamedQueryCompletion, ExecutionError>>,
}

pub struct DocumentCancellationHandle {
    task: tokio::task::JoinHandle<Result<(), ExecutionError>>,
}

impl DocumentCancellationHandle {
    pub async fn wait(self) -> Result<(), ExecutionError> {
        self.task.await.map_err(|error| {
            ExecutionError::Message(format!("Cancellation worker failed: {error}"))
        })?
    }
}

impl DocumentQueryHandle {
    pub async fn wait(self) -> Result<StreamedQueryCompletion, ExecutionError> {
        self.task
            .await
            .map_err(|error| ExecutionError::Message(format!("Document worker failed: {error}")))?
    }
}

impl QueryDocument {
    pub fn submit(
        &self,
        engine: Arc<QueryEngine>,
        sql: String,
        limits: ResultLimits,
        history: Option<QueryHistoryStore>,
    ) -> Result<DocumentQueryHandle, ExecutionError> {
        if sql.trim().is_empty() {
            return Err(ExecutionError::Message("Empty query".into()));
        }
        let mut admitted = Err(ExecutionError::Closed);
        self.mutate(|state| {
            if state.lifecycle != DocumentLifecycle::Open {
                return false;
            }
            if state.execution.is_active() {
                admitted = Err(ExecutionError::Busy);
                return false;
            }
            let query = match engine.start_streamed(sql, limits, history) {
                Ok(query) => query,
                Err(error) => {
                    admitted = Err(error);
                    return false;
                }
            };
            state.execution = DocumentExecution::Planning(query.id);
            state.session = Some(query.session);
            state.output_generation = state.output_generation.checked_add(1).unwrap();
            state.plan = None;
            state.operation = None;
            state.completion = None;
            admitted = Ok((query, state.id, state.output_generation));
            true
        });
        let (query, document, generation) = admitted?;
        let id = query.id;
        let session = query.session;
        let mut progress = engine.subscribe();
        let target = self.clone();
        let task = tokio::spawn(async move {
            let completion = query.wait_completion();
            tokio::pin!(completion);
            let mut progress_open = true;
            loop {
                let snapshot = progress.borrow_and_update().clone();
                if snapshot.session == session && snapshot.phase.id() == Some(id) {
                    target.mutate(|state| {
                        if state.lifecycle != DocumentLifecycle::Open
                            || state.execution.query() != Some(id)
                        {
                            return false;
                        }
                        state.plan = snapshot.plan.clone();
                        state.execution = match snapshot.phase {
                            QueryPhase::Queued(_) | QueryPhase::Explaining(_)
                                if state.execution != DocumentExecution::Canceling(id) =>
                            {
                                DocumentExecution::Planning(id)
                            }
                            QueryPhase::Canceling(_) => DocumentExecution::Canceling(id),
                            QueryPhase::Executing(_)
                                if state.execution != DocumentExecution::Canceling(id) =>
                            {
                                DocumentExecution::Running(id)
                            }
                            _ => state.execution,
                        };
                        state.operation = Some(snapshot);
                        true
                    });
                }
                tokio::select! {
                    result = &mut completion => {
                        target.mutate(|state| {
                            if state.lifecycle != DocumentLifecycle::Open
                                || state.execution.query() != Some(id) {
                                return false;
                            }
                            state.execution = DocumentExecution::Finished(id);
                            state.completion = Some(result.as_ref()
                                .map(|completion| Arc::new(completion.clone()))
                                .map_err(Clone::clone));
                            if let Ok(completion) = &result {
                                state.plan = completion.snapshot.plan.clone();
                                state.operation = Some(completion.snapshot.clone());
                            }
                            true
                        });
                        return result;
                    }
                    changed = progress.changed(), if progress_open => {
                        progress_open = changed.is_ok();
                    }
                }
            }
        });
        Ok(DocumentQueryHandle {
            document,
            query: id,
            session,
            generation,
            task,
        })
    }

    pub fn request_cancel(
        &self,
        engine: Arc<QueryEngine>,
    ) -> Result<DocumentCancellationHandle, ExecutionError> {
        let mut requested = Err(ExecutionError::Busy);
        self.mutate(|state| {
            if state.lifecycle != DocumentLifecycle::Open {
                requested = Err(ExecutionError::Closed);
                return false;
            }
            if state.session != Some(engine.id()) {
                requested = Err(ExecutionError::Message(
                    "Query belongs to another session".into(),
                ));
                return false;
            }
            let (DocumentExecution::Planning(query) | DocumentExecution::Running(query)) =
                state.execution
            else {
                return false;
            };
            state.execution = DocumentExecution::Canceling(query);
            requested = Ok(query);
            true
        });
        let query = requested?;
        let document = self.clone();
        let task = tokio::spawn(async move {
            let result = engine.cancel_detailed(query).await;
            if result.is_err() {
                document.cancel_failed(query);
            }
            result
        });
        Ok(DocumentCancellationHandle { task })
    }
}
