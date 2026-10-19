use std::future::Future;
use std::sync::Arc;

use dzban_core::application::ApplicationContext;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dropped;

impl std::fmt::Display for Dropped {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Background work was dropped")
    }
}

#[derive(Clone)]
pub struct RuntimeBridge {
    runtime: Arc<tokio::runtime::Runtime>,
    context: Arc<ApplicationContext>,
}

impl std::fmt::Debug for RuntimeBridge {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeBridge")
            .finish_non_exhaustive()
    }
}

impl RuntimeBridge {
    pub fn new() -> Result<Self, String> {
        Self::with_context(Arc::new(ApplicationContext::new(
            dzban_db::adapters::registry(),
        )))
    }

    pub fn with_context(context: Arc<ApplicationContext>) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("dzban-tokio")
            .enable_all()
            .build()
            .map_err(|error| format!("Failed to create Tokio runtime: {error}"))?;
        Ok(Self {
            runtime: Arc::new(runtime),
            context,
        })
    }

    pub fn context(&self) -> &Arc<ApplicationContext> {
        &self.context
    }

    pub fn enter(&self) -> tokio::runtime::EnterGuard<'_> {
        self.runtime.enter()
    }

    pub fn run<F>(
        &self,
        future: F,
    ) -> impl Future<Output = Result<F::Output, Dropped>> + 'static + use<F>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.runtime.spawn(async move {
            let _ = sender.send(future.await);
        });
        async move { receiver.await.map_err(|_| Dropped) }
    }
}
