use std::sync::Arc;

/// Represents a type that has
pub trait DatabaseMetrics {
    /// Reports metrics for the database.
    fn report_metrics(&self);
}

impl<DB: DatabaseMetrics> DatabaseMetrics for Arc<DB> {
    fn report_metrics(&self) {
        <DB as DatabaseMetrics>::report_metrics(self)
    }
}
