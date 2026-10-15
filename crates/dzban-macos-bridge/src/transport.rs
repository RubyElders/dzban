use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::watch;

pub const MAX_RECORD_BYTES: usize = 8 * 1024 * 1024;

#[derive(Default)]
struct State {
    revision: u64,
    record: Option<Vec<u8>>,
    closed: bool,
    waiting: bool,
}

pub struct Stream {
    state: Mutex<State>,
    ready: Condvar,
    closed: watch::Sender<bool>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Wait {
    Record(Vec<u8>),
    Timeout,
    Closed,
    AlreadyWaiting,
}

impl Stream {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State::default()),
            ready: Condvar::new(),
            closed: watch::channel(false).0,
        })
    }

    pub fn publish(&self, payload: Value) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.closed {
            return;
        }
        state.revision += 1;
        let mut record = serde_json::to_vec(&json!({
            "schema": 1,
            "revision": state.revision,
            "payload": payload,
        }))
        .expect("JSON values serialize");
        if record.len() > MAX_RECORD_BYTES {
            record = serde_json::to_vec(&json!({
                "schema": 1,
                "revision": state.revision,
                "payload": {"failure": {"kind": "record_too_large", "message": "Record exceeds the bridge byte limit"}},
            }))
            .expect("JSON values serialize");
        }
        state.record = Some(record);
        self.ready.notify_all();
    }

    pub fn wait(&self, after: u64, timeout: Duration) -> Wait {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.waiting {
            return Wait::AlreadyWaiting;
        }
        state.waiting = true;
        let (mut state, _) = self
            .ready
            .wait_timeout_while(state, timeout, |state| {
                !state.closed && state.revision <= after
            })
            .unwrap_or_else(|error| error.into_inner());
        state.waiting = false;
        if state.closed {
            Wait::Closed
        } else if state.revision > after {
            Wait::Record(
                state
                    .record
                    .clone()
                    .expect("published revision has a record"),
            )
        } else {
            Wait::Timeout
        }
    }

    pub fn close(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.closed = true;
        state.record = None;
        self.closed.send_replace(true);
        self.ready.notify_all();
    }

    pub async fn closed(&self) {
        let mut receiver = self.closed.subscribe();
        receiver.wait_for(|closed| *closed).await.ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivery_coalesces_and_retains_the_latest_completion() {
        let stream = Stream::new();
        for row in 0..10_000 {
            stream.publish(json!({"rows": row}));
        }
        let Wait::Record(record) = stream.wait(0, Duration::ZERO) else {
            panic!("missing retained state");
        };
        let record: Value = serde_json::from_slice(&record).unwrap();
        assert_eq!(record["revision"], 10_000);
        assert_eq!(record["payload"]["rows"], 9_999);
        assert_eq!(stream.wait(10_000, Duration::ZERO), Wait::Timeout);
    }

    #[test]
    fn releasing_a_stream_wakes_its_waiter_and_rejects_publication() {
        let stream = Stream::new();
        let waiter = stream.clone();
        let thread = std::thread::spawn(move || waiter.wait(0, Duration::from_secs(30)));
        stream.close();
        assert_eq!(thread.join().unwrap(), Wait::Closed);
        stream.publish(json!({"late": true}));
        assert_eq!(stream.wait(0, Duration::ZERO), Wait::Closed);
    }

    #[test]
    fn simultaneous_waiters_receive_an_explicit_error() {
        let stream = Stream::new();
        let waiter = stream.clone();
        let thread = std::thread::spawn(move || waiter.wait(0, Duration::from_secs(30)));
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while !stream.state.lock().unwrap().waiting {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(stream.wait(0, Duration::ZERO), Wait::AlreadyWaiting);
        stream.close();
        assert_eq!(thread.join().unwrap(), Wait::Closed);
    }

    #[tokio::test]
    async fn asynchronous_observers_also_see_release_before_subscription() {
        let stream = Stream::new();
        stream.close();
        tokio::time::timeout(Duration::from_secs(1), stream.closed())
            .await
            .unwrap();
    }

    #[test]
    fn oversized_records_report_an_explicit_bounded_error() {
        let stream = Stream::new();
        stream.publish(json!({"value": "x".repeat(MAX_RECORD_BYTES)}));
        let Wait::Record(record) = stream.wait(0, Duration::ZERO) else {
            panic!("missing overflow error");
        };
        assert!(record.len() < 1024);
        let record: Value = serde_json::from_slice(&record).unwrap();
        assert_eq!(record["payload"]["failure"]["kind"], "record_too_large");
    }
}
