//! tokio-backed [`TaskSpawner`].

use es_runtime_providers::{BoxFuture, TaskSpawner};

/// A [`TaskSpawner`] that offloads blocking work to tokio's blocking pool.
pub struct TokioTaskSpawner;

impl TaskSpawner for TokioTaskSpawner {
    fn spawn_blocking(&self, work: Box<dyn FnOnce() + Send + 'static>) -> BoxFuture<()> {
        Box::pin(async move {
            // A join error means the task panicked or was aborted; the work's
            // own outputs (via captured channels) convey success/failure, so the
            // join result is intentionally discarded here.
            let _ = tokio::task::spawn_blocking(work).await;
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test]
    async fn runs_the_closure_off_thread() {
        let ran = Arc::new(AtomicBool::new(false));
        let flag = ran.clone();
        TokioTaskSpawner
            .spawn_blocking(Box::new(move || flag.store(true, Ordering::SeqCst)))
            .await;
        assert!(ran.load(Ordering::SeqCst));
    }
}
