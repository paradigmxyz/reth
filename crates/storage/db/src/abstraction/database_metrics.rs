use std::sync::Arc;

use crate::database::Database;

/// Extends [Database], adding a function that can be used as a hook for metric reporting.
pub trait DatabaseMetrics: Database {
    /// Reports metrics for the database.
    fn report_metrics(&self);
}

impl<DB: DatabaseMetrics> DatabaseMetrics for Arc<DB> {
    fn report_metrics(&self) {
        <DB as DatabaseMetrics>::report_metrics(self)
    }
}
