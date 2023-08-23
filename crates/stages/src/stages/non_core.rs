//! A non core stage is one that is not required for node functionality.
//!
//! Users may define additional stages and activate them via flags.
//! Implementation is achieved by implementing the NonCoreStage trait on a new struct.
//!
//! Non core stages may also define new non-core database tables and associate them here.

use async_trait::async_trait;
use reth_db::database::Database;

use crate::Stage;

/// A collection of methods and types that make up a non-core Stage.
#[async_trait]
pub trait NonCoreStage<DB>
where
    Self: Stage<DB>,
    DB: Database,
{
    /// A config struct used to set up the stage.
    type Config;
    
    /// Used by the pipline builder to set the new stage.
    fn new() -> Self;
}
