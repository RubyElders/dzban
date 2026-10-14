use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, oneshot, watch};

use crate::error::ExecutionError;
use crate::result_store::{ResultCutoff, ResultLimits, ResultStore};
use crate::{ResultCell, ResultColumn, ResultSet};

#[derive(Clone)]
pub struct ResultHandle(Arc<Mutex<ResultStore>>);

impl std::fmt::Debug for ResultHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResultHandle")
            .finish_non_exhaustive()
    }
}

impl ResultHandle {
    pub async fn read_page(&self, start: usize, count: usize) -> Result<ResultSet, ExecutionError> {
        let store = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let mut store = store.lock().map_err(storage_error)?;
            Ok(ResultSet {
                columns: store.columns().to_vec(),
                rows: store.read_page(start, count).map_err(storage_error)?,
            })
        })
        .await
        .map_err(storage_error)?
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResultTermination {
    Complete,
    Limited(ResultCutoff),
    Canceled,
    Failed(ExecutionError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResultPhase {
    WaitingForSchema,
    Receiving,
    Draining(ResultCutoff),
    FailedAwaitingCleanup(ExecutionError),
    Finished(ResultTermination),
}

#[derive(Clone, Debug)]
pub struct ResultSnapshot {
    pub phase: ResultPhase,
    pub result: Option<ResultHandle>,
    pub rows: usize,
    pub received_rows: usize,
    pub bytes: usize,
    pub cutoff: Option<ResultCutoff>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    Continue,
    Stop(ResultCutoff),
}

enum Event {
    Schema(Vec<ResultColumn>),
    Row(Vec<ResultCell>),
    OversizedRow,
    Finish(Result<(), ExecutionError>),
}

struct Request {
    event: Event,
    reply: oneshot::Sender<Result<Delivery, ExecutionError>>,
}

pub struct ResultSink(mpsc::Sender<Request>);

impl ResultSink {
    pub async fn schema(&mut self, columns: Vec<ResultColumn>) -> Result<Delivery, ExecutionError> {
        self.deliver(Event::Schema(columns)).await
    }

    pub async fn row(&mut self, row: Vec<ResultCell>) -> Result<Delivery, ExecutionError> {
        self.deliver(Event::Row(row)).await
    }

    pub async fn oversized_row(&mut self) -> Result<Delivery, ExecutionError> {
        self.deliver(Event::OversizedRow).await
    }

    pub async fn finish(
        &mut self,
        outcome: Result<(), ExecutionError>,
    ) -> Result<(), ExecutionError> {
        self.deliver(Event::Finish(outcome)).await.map(|_| ())
    }

    async fn deliver(&mut self, event: Event) -> Result<Delivery, ExecutionError> {
        let (reply, response) = oneshot::channel();
        self.0
            .send(Request { event, reply })
            .await
            .map_err(storage_error)?;
        response.await.map_err(storage_error)?
    }
}

pub fn start_results(limits: ResultLimits) -> (ResultSink, watch::Receiver<ResultSnapshot>) {
    let (requests, mut incoming) = mpsc::channel::<Request>(1);
    let (snapshots, reader) = watch::channel(ResultSnapshot {
        phase: ResultPhase::WaitingForSchema,
        result: None,
        rows: 0,
        received_rows: 0,
        bytes: 0,
        cutoff: None,
    });
    tokio::task::spawn_blocking(move || {
        let mut current = snapshots.borrow().clone();
        while let Some(request) = incoming.blocking_recv() {
            let finish = matches!(request.event, Event::Finish(_));
            let result = process(request.event, limits, &mut current);
            if let Err(error) = &result {
                current.phase = ResultPhase::FailedAwaitingCleanup(error.clone());
            }
            snapshots.send_replace(current.clone());
            let _ = request.reply.send(result);
            if finish {
                return;
            }
        }
        let error = match current.phase {
            ResultPhase::FailedAwaitingCleanup(error) => error,
            _ => ExecutionError::Message("Result producer closed without completing".into()),
        };
        current.phase = ResultPhase::Finished(ResultTermination::Failed(error));
        snapshots.send_replace(current);
    });
    (ResultSink(requests), reader)
}

fn process(
    event: Event,
    limits: ResultLimits,
    current: &mut ResultSnapshot,
) -> Result<Delivery, ExecutionError> {
    if let ResultPhase::FailedAwaitingCleanup(error) = &current.phase {
        if matches!(event, Event::Finish(_)) {
            current.phase = ResultPhase::Finished(ResultTermination::Failed(error.clone()));
            return Ok(Delivery::Continue);
        }
        return Err(error.clone());
    }
    match event {
        Event::Schema(columns) => {
            if current.result.is_some() {
                return Err(ExecutionError::Message(
                    "Result schema was already received".into(),
                ));
            }
            let store = ResultStore::new(columns, limits).map_err(storage_error)?;
            current.bytes = store.bytes();
            let cutoff = store.cutoff();
            current.cutoff = cutoff;
            current.result = Some(ResultHandle(Arc::new(Mutex::new(store))));
            current.phase = cutoff.map_or(ResultPhase::Receiving, ResultPhase::Draining);
            Ok(cutoff.map_or(Delivery::Continue, Delivery::Stop))
        }
        Event::Row(_) | Event::OversizedRow => {
            current.received_rows = current
                .received_rows
                .checked_add(1)
                .ok_or_else(|| ExecutionError::Message("Result row count overflow".into()))?;
            let result = current.result.as_ref().ok_or_else(|| {
                ExecutionError::Message("Result row arrived before its schema".into())
            })?;
            let mut store = result.0.lock().map_err(storage_error)?;
            let cutoff = match event {
                Event::Row(row) => store.push(row).map_err(storage_error)?,
                Event::OversizedRow => Some(store.reject_oversized_row()),
                _ => unreachable!(),
            };
            current.cutoff = cutoff;
            current.rows = store.len();
            current.bytes = store.bytes();
            current.phase = cutoff.map_or(ResultPhase::Receiving, ResultPhase::Draining);
            Ok(cutoff.map_or(Delivery::Continue, Delivery::Stop))
        }
        Event::Finish(outcome) => {
            let termination = match outcome {
                Ok(()) => match current.phase {
                    ResultPhase::Draining(cutoff) => ResultTermination::Limited(cutoff),
                    _ => ResultTermination::Complete,
                },
                Err(ExecutionError::Canceled) => ResultTermination::Canceled,
                Err(error) => ResultTermination::Failed(error),
            };
            current.phase = ResultPhase::Finished(termination);
            Ok(Delivery::Continue)
        }
    }
}

fn storage_error(error: impl std::fmt::Display) -> ExecutionError {
    ExecutionError::Message(format!("Result storage: {error}"))
}
