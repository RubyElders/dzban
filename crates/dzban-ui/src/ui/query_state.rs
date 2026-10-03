use std::sync::{Arc, Mutex};

/// Represents the state of a query execution
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryState {
    Idle,
    Running,
    Canceling,
}

/// Tracks execution and cancellation state for one query tab.
#[derive(Clone)]
pub struct QueryStateManager {
    state: Arc<Mutex<QueryState>>,
}

#[derive(Clone)]
pub struct QueryExecution {
    query_num: u32,
    state_manager: QueryStateManager,
}

impl QueryExecution {
    pub fn start(query_num: u32, state_manager: QueryStateManager) -> Self {
        state_manager.start();
        Self {
            query_num,
            state_manager,
        }
    }

    pub fn query_num(&self) -> u32 {
        self.query_num
    }

    pub fn finish(&self) {
        self.state_manager.finish();
    }
}

impl QueryStateManager {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(QueryState::Idle)),
        }
    }

    pub fn start(&self) {
        let mut state = self.state.lock().unwrap();
        *state = QueryState::Running;
    }

    /// Mark query as finished
    pub fn finish(&self) {
        let mut state = self.state.lock().unwrap();
        *state = QueryState::Idle;
    }

    /// Request cancellation of running query
    pub fn cancel(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if *state == QueryState::Running {
            *state = QueryState::Canceling;
            true
        } else {
            false
        }
    }

    pub fn cancel_failed(&self) {
        let mut state = self.state.lock().unwrap();
        if *state == QueryState::Canceling {
            *state = QueryState::Running;
        }
    }
}

impl Default for QueryStateManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_cancellation_keeps_query_running() {
        let state = QueryStateManager::new();
        state.start();
        assert!(state.cancel());

        state.cancel_failed();

        assert!(state.cancel());
        state.finish();
        state.start();
        assert!(state.cancel());
    }

    #[test]
    fn completion_finishes_the_execution_that_started_it() {
        let first = QueryStateManager::new();
        let second = QueryStateManager::new();
        let execution = QueryExecution::start(1, first.clone());
        QueryExecution::start(2, second.clone());

        execution.finish();

        assert_eq!(execution.query_num(), 1);
        assert!(!first.cancel());
        assert!(second.cancel());
    }
}
