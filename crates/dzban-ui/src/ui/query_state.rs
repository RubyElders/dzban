use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

/// Represents the state of a query execution
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryState {
    Idle,
    Running,
    Canceling,
}

/// Manages query execution state to prevent concurrent queries
#[derive(Clone)]
pub struct QueryStateManager {
    state: Arc<Mutex<QueryState>>,
    cancel_flag: Arc<AtomicBool>,
}

impl QueryStateManager {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(QueryState::Idle)),
            cancel_flag: Arc::new(AtomicBool::new(false)),
        }
    }
    
    /// Try to start a query. Returns true if started, false if already running
    pub fn try_start(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if *state == QueryState::Idle {
            *state = QueryState::Running;
            self.cancel_flag.store(false, Ordering::SeqCst);
            true
        } else {
            false
        }
    }
    
    /// Mark query as finished
    pub fn finish(&self) {
        let mut state = self.state.lock().unwrap();
        *state = QueryState::Idle;
        self.cancel_flag.store(false, Ordering::SeqCst);
    }
    
    /// Request cancellation of running query
    pub fn cancel(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if *state == QueryState::Running {
            *state = QueryState::Canceling;
            self.cancel_flag.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }
    
    /// Get current state
    pub fn get_state(&self) -> QueryState {
        *self.state.lock().unwrap()
    }
    
    /// Check if query is running
    pub fn is_running(&self) -> bool {
        *self.state.lock().unwrap() == QueryState::Running
    }
    
    /// Check if cancellation was requested
    pub fn is_canceled(&self) -> bool {
        self.cancel_flag.load(Ordering::SeqCst)
    }
}

impl Default for QueryStateManager {
    fn default() -> Self {
        Self::new()
    }
}
