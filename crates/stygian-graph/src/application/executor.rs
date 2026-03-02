//! Pipeline executor

/// Pipeline executor for orchestrating scraping operations
pub struct PipelineExecutor;

impl PipelineExecutor {
    /// Create a new pipeline executor
    pub const fn new() -> Self {
        Self
    }
}

impl Default for PipelineExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn new_returns_executor() {
        let _e = PipelineExecutor::new();
    }

    #[test]
    fn default_is_same_as_new() {
        let _e = PipelineExecutor::default();
    }
}
