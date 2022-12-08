# Stages

The `stages` lib plays a central role in syncing the node, maintaining state, updating the database and more. The stages involved in the Reth pipeline are the `HeaderStage`, `BodyStage`, `SendersStage`, and `ExecutionStage`. Each of these stages are queued up and stored within the Reth pipeline.

Filename: crates/stages/src/pipeline.rs
```rust
pub struct Pipeline<DB: Database> {
    stages: Vec<QueuedStage<DB>>,
    max_block: Option<BlockNumber>,
    events_sender: MaybeSender<PipelineEvent>,
}
```

When the node is first started, a new `Pipeline` is intialized and all of the stages are added into `Pipline.stages`. Then, the `Pipeline::run` function is called, which starts the pipline, executing all of the stages continuously in an infinite loop. This process syncs the chain, keeping everything up to date with the chain tip. 
Each stage within the pipeline implements the `Stage` trait which provides function interfaces to get the stage id, execute the stage and unwind the state if there was an issue during the stage execution.


Filename: crates/stages/src/stage.rs
```rust
#[async_trait]
pub trait Stage<DB: Database>: Send + Sync {
    /// Get the ID of the stage.
    ///
    /// Stage IDs must be unique.
    fn id(&self) -> StageId;

    /// Execute the stage.
    async fn execute(
        &mut self,
        db: &mut StageDB<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError>;

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        db: &mut StageDB<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, Box<dyn std::error::Error + Send + Sync>>;
}
```

To get a better idea of what is happening at each part of the pipeline, lets walk through what is going on under the hood within the `execution()` function at each stage, starting with `HeadersStage`.

## HeadersStage

The `HeadersStage` is responsible for syncing the block headers, validating the header integrity and writing the headers to the database. When the `execute()` function is called, the head of the chain is updated to the most recent block height previously executed by the stage. At this point, the node status is also updated with the most recent block height, hash and total difficulty. These values are used during any new eth/65 handshakes. After updating the head, a stream is established with other peers in the network to sync the missing chain headers between the most recent state stored in the database and the chain tip. This stage relies on the stream to return the headers in descending order staring from the chain tip down to the latest block in the database.

Each value produced from the stream is a `Vec<SealedHeader>`. 

File: crates/primitives/src/header.rs
```rust
/// A [`Header`] that is sealed at a precalculated hash, use [`SealedHeader::unseal()`] if you want
/// to modify header.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SealedHeader {
    /// Locked Header fields.
    header: Header,
    /// Locked Header hash.
    hash: BlockHash,
}
```

Each `SealedHeader` is then validated to ensure that it has the proper parent, where each header is then written to the database proceeding validation. If a header is not valid or the stream encounters any other error, the error is propagated up through the stage execution, the db changes are unwound and the stage is resumed from the most recent valid state.

This process continues until all of the headers have been downloaded and and written to the database. Finally, the total difficulty of the chain's head is updated and the function returns `Ok(ExecOutput { stage_progress: current_progress, reached_tip: true, done: true })`, signaling that the header sync has completed successfuly. 


## BodyStage


## SendersStage


## ExecutionStage





