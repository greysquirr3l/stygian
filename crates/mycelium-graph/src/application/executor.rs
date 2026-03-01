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
