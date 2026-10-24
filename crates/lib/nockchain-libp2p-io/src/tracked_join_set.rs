use std::collections::HashMap;
use std::future::Future;

use tokio::task::{AbortHandle, JoinError, JoinSet};

pub(crate) struct TrackedJoinSet<T> {
    inner: JoinSet<T>,
    tasks: HashMap<String, AbortHandle>,
}

impl<T: 'static> TrackedJoinSet<T> {
    pub(crate) fn new() -> Self {
        Self {
            inner: JoinSet::new(),
            tasks: HashMap::new(),
        }
    }

    pub(crate) fn spawn(&mut self, name: String, task: impl Future<Output = T> + Send + 'static)
    where
        T: Send + 'static,
    {
        let handle = self.inner.spawn(task);
        self.tasks.insert(name, handle);
    }

    pub(crate) async fn join_next(&mut self) -> Option<Result<T, JoinError>> {
        let result = self.inner.join_next().await;
        if result.is_some() {
            // Remove the completed task from our tracking
            self.tasks.retain(|_, v| !v.is_finished());
        }
        result
    }

    // Keep this around for debugging
    #[allow(dead_code)]
    pub(crate) fn get_running_tasks(&self) -> Vec<String> {
        self.tasks.keys().cloned().collect()
    }
}
