use crate::Stage;
use reth_primitives::U64;
use std::fmt::{Debug, Formatter};

#[allow(dead_code)]
struct QueuedStage {
    /// The actual stage to execute.
    stage: Box<dyn Stage>,
    /// The unwind priority of the stage.
    unwind_priority: usize,
    /// Whether or not this stage can only execute when we reach what we believe to be the tip of
    /// the chain.
    require_tip: bool,
}

/// A staged sync pipeline.
///
/// The pipeline executes queued [stages][Stage] serially. An external component determines the tip
/// of the chain and the pipeline then executes each stage in order from the current local chain tip
/// and the external chain tip. When a stage is executed, it will run until it reaches the chain
/// tip.
///
/// After the entire pipeline has been run, it will run again unless asked to stop (see
/// [Pipeline::set_exit_after_sync]).
///
/// # Unwinding
///
/// In case of a validation error (as determined by the consensus engine) in one of the stages, the
/// pipeline will unwind the stages according to their unwind priority. It is also possible to
/// request an unwind manually (see [Pipeline::start_with_unwind]).
///
/// The unwind priority is set with [Pipeline::push_with_unwind_priority]. Stages with higher unwind
/// priorities are unwound first.
#[derive(Default)]
pub struct Pipeline {
    stages: Vec<QueuedStage>,
    unwind_to: Option<U64>,
    max_block: Option<U64>,
    exit_after_sync: bool,
}

impl Debug for Pipeline {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pipeline")
            .field("unwind_to", &self.unwind_to)
            .field("max_block", &self.max_block)
            .field("exit_after_sync", &self.exit_after_sync)
            .finish()
    }
}

impl Pipeline {
    /// Add a stage to the pipeline.
    ///
    /// # Unwinding
    ///
    /// The unwind priority is set to 0.
    pub fn push<S>(&mut self, stage: S, require_tip: bool) -> &mut Self
    where
        S: Stage + 'static,
    {
        self.push_with_unwind_priority(stage, require_tip, 0)
    }

    /// Add a stage to the pipeline, specifying the unwind priority.
    pub fn push_with_unwind_priority<S>(
        &mut self,
        stage: S,
        require_tip: bool,
        unwind_priority: usize,
    ) -> &mut Self
    where
        S: Stage + 'static,
    {
        self.stages.push(QueuedStage { stage: Box::new(stage), require_tip, unwind_priority });
        self
    }

    /// Set the target block.
    ///
    /// Once this block is reached, syncing will stop.
    pub fn set_max_block(&mut self, block: Option<U64>) -> &mut Self {
        self.max_block = block;
        self
    }

    /// Start the pipeline by unwinding to the specified block.
    pub fn start_with_unwind(&mut self, unwind_to: Option<U64>) -> &mut Self {
        self.unwind_to = unwind_to;
        self
    }

    /// Control whether the pipeline should exit after syncing.
    pub fn set_exit_after_sync(&mut self, exit: bool) -> &mut Self {
        self.exit_after_sync = exit;
        self
    }

    /// Run the pipeline.
    pub async fn run(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        todo!()
    }
}
