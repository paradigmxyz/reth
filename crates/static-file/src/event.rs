use crate::StaticFileTargets;
use std::time::Duration;

/// An event emitted by a [StaticFileProducer][crate::StaticFileProducer].
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum StaticFileProducerEvent {
    /// Emitted when static file producer started running.
    Started {
        /// Targets that will be moved to static files
        targets: StaticFileTargets,
    },
    /// Emitted when static file producer finished running.
    Finished {
        /// Targets that were moved to static files
        targets: StaticFileTargets,
        /// Time it took to run the static file producer
        elapsed: Duration,
    },
}
