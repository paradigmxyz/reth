# Stages

The `stages` lib plays a central role in syncing the node, maintaining state, updating the database and more. The stages involved in the Reth pipeline are the `HeaderStage`, `BodyStage`, `SenderRecoveryStage`, and `ExecutionStage` (note that this list is non-exhaustive, and more pipeline stages will be added in the near future). Each of these stages are queued up and stored within the Reth pipeline.

[File: crates/stages/src/pipeline.rs](https://github.com/paradigmxyz/reth/blob/main/crates/stages/src/pipeline.rs)
```rust,ignore
pub struct Pipeline<DB: Database, U: SyncStateUpdater> {
    stages: Vec<BoxedStage<DB>>,
    max_block: Option<BlockNumber>,
    listeners: PipelineEventListeners,
    sync_state_updater: Option<U>,
    progress: PipelineProgress,
    metrics: Metrics,
}
```


When the node is first started, a new `Pipeline` is initialized and all of the stages are added into `Pipeline.stages`. Then, the `Pipeline::run` function is called, which starts the pipeline, executing all of the stages continuously in an infinite loop. This process syncs the chain, keeping everything up to date with the chain tip. 

Each stage within the pipeline implements the `Stage` trait which provides function interfaces to get the stage id, execute the stage and unwind the changes to the database if there was an issue during the stage execution.

[File: crates/stages/src/stage.rs](https://github.com/paradigmxyz/reth/blob/main/crates/stages/src/stage.rs)
```rust,ignore
pub trait Stage<DB: Database>: Send + Sync {
    /// Get the ID of the stage.
    ///
    /// Stage IDs must be unique.
    fn id(&self) -> StageId;

    /// Execute the stage.
    async fn execute(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError>;

    /// Unwind the stage.
    async fn unwind(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError>;
}
```

To get a better idea of what is happening at each part of the pipeline, lets walk through what is going on under the hood within the `execute()` function at each stage, starting with `HeaderStage`.

<br>


## HeaderStage

<!-- TODO: Cross-link to eth/65 chapter when it's written -->
The `HeaderStage` is responsible for syncing the block headers, validating the header integrity and writing the headers to the database. When the `execute()` function is called, the local head of the chain is updated to the most recent block height previously executed by the stage. At this point, the node status is also updated with that block's height, hash and total difficulty. These values are used during any new eth/65 handshakes. After updating the head, a stream is established with other peers in the network to sync the missing chain headers between the most recent state stored in the database and the chain tip. The `HeaderStage` contains a `downloader` attribute, which is a type that implements the `HeaderDownloader` trait. A `HeaderDownloader` is a `Stream` that returns batches of headers.

[File: crates/interfaces/src/p2p/headers/downloader.rs](https://github.com/paradigmxyz/reth/blob/main/crates/interfaces/src/p2p/headers/downloader.rs)
```rust,ignore
pub trait HeaderDownloader: Send + Sync + Stream<Item = Vec<SealedHeader>> + Unpin {
    /// Updates the gap to sync which ranges from local head to the sync target
    ///
    /// See also [HeaderDownloader::update_sync_target] and [HeaderDownloader::update_local_head]
    fn update_sync_gap(&mut self, head: SealedHeader, target: SyncTarget) {
        self.update_local_head(head);
        self.update_sync_target(target);
    }

    /// Updates the block number of the local database
    fn update_local_head(&mut self, head: SealedHeader);

    /// Updates the target we want to sync to
    fn update_sync_target(&mut self, target: SyncTarget);

    /// Sets the headers batch size that the Stream should return.
    fn set_batch_size(&mut self, limit: usize);
}
```

The `HeaderStage` relies on the downloader stream to return the headers in descending order starting from the chain tip down to the latest block in the database. While other stages in the `Pipeline` start from the most recent block in the database up to the chain tip, the `HeaderStage` works in reverse to avoid [long-range attacks](https://messari.io/report/long-range-attack). When a node downloads headers in ascending order, it will not know if it is being subjected to a long-range attack until it reaches the most recent blocks. To combat this, the `HeaderStage` starts by getting the chain tip from the Consensus Layer, verifies the tip, and then walks backwards by the parent hash. Each value yielded from the stream is a `SealedHeader`. 

[File: crates/primitives/src/header.rs](https://github.com/paradigmxyz/reth/blob/main/crates/primitives/src/header.rs)
```rust,ignore
pub struct SealedHeader {
    /// Locked Header fields.
    header: Header,
    /// Locked Header hash.
    hash: BlockHash,
}
```

<!-- Not sure about this, how is it validated? -->
Each `SealedHeader` is then validated to ensure that it has the proper parent. Note that this is only a basic response validation, and the `HeaderDownloader` uses the `validate` method during the `stream`, so that each header is validated according to the consensus specification before the header is yielded from the stream. After this, each header is then written to the database. If a header is not valid or the stream encounters any other error, the error is propagated up through the stage execution, the changes to the database are unwound and the stage is resumed from the most recent valid state.

This process continues until all of the headers have been downloaded and written to the database. Finally, the total difficulty of the chain's head is updated and the function returns `Ok(ExecOutput { stage_progress, done: true })`, signaling that the header sync has completed successfully. 

<br>

## TotalDifficultyStage
* TODO: explain stage
<br>

## BodyStage

Once the `HeaderStage` completes successfully, the `BodyStage` will start execution. The body stage downloads block bodies for all of the new block headers that were stored locally in the database. The `BodyStage` first determines which block bodies to download by checking if the block body has an ommers hash and transaction root. 

An ommers hash is the Keccak 256-bit hash of the ommers list portion of the block. If you are unfamiliar with ommers blocks, you can [click here to learn more](https://ethereum.org/en/glossary/#ommer). Note that while ommers blocks were important for new blocks created during Ethereum's proof of work chain, Ethereum's proof of stake chain selects exactly one block proposer at a time, causing ommers blocks not to be needed in post-merge Ethereum.

The transactions root is a value that is calculated based on the transactions included in the block. To derive the transactions root, a [merkle tree](https://blog.ethereum.org/2015/11/15/merkling-in-ethereum) is created from the block's transactions list. The transactions root is then derived by taking the Keccak 256-bit hash of the root node of the merkle tree.

When the `BodyStage` is looking at the headers to determine which block to download, it will skip the blocks where the `header.ommers_hash` and the `header.transaction_root` are empty, denoting that the block is empty as well.

Once the `BodyStage` determines which block bodies to fetch, a new `bodies_stream` is created which downloads all of the bodies from the `starting_block`, up until the `target_block` specified. Each time the `bodies_stream` yields a value, a `SealedBlock` is created using the block header, the ommers hash and the newly downloaded block body.

[File: crates/primitives/src/block.rs](https://github.com/paradigmxyz/reth/blob/main/crates/primitives/src/block.rs)
```rust,ignore
pub struct SealedBlock {
    /// Locked block header.
    pub header: SealedHeader,
    /// Transactions with signatures.
    pub body: Vec<TransactionSigned>,
    /// Ommer/uncle headers
    pub ommers: Vec<SealedHeader>,
}
```

The new block is then pre-validated, checking that the ommers hash and transactions root in the block header are the same in the block body. Following a successful pre-validation, the `BodyStage` loops through each transaction in the `block.body`, adding the transaction to the database. This process is repeated for every downloaded block body, with the `BodyStage` returning `Ok(ExecOutput { stage_progress, done: true })` signaling it successfully completed. 

<br>

## SenderRecoveryStage

Following a successful `BodyStage`, the `SenderRecoveryStage` starts to execute. The `SenderRecoveryStage` is responsible for recovering the transaction sender for each of the newly added transactions to the database. At the beginning of the execution function, all of the transactions are first retrieved from the database. Then the `SenderRecoveryStage` goes through each transaction and recovers the signer from the transaction signature and hash. The transaction hash is derived by taking the Keccak 256-bit hash of the RLP encoded transaction bytes. This hash is then passed into the `recover_signer` function.

[File: crates/primitives/src/transaction/signature.rs](https://github.com/paradigmxyz/reth/blob/main/crates/primitives/src/transaction/signature.rs)
```rust,ignore
pub(crate) fn recover_signer(&self, hash: H256) -> Option<Address> {
    let mut sig: [u8; 65] = [0; 65];

    sig[0..32].copy_from_slice(&self.r.to_be_bytes::<32>());
    sig[32..64].copy_from_slice(&self.s.to_be_bytes::<32>());
    sig[64] = self.odd_y_parity as u8;

    // NOTE: we are removing error from underlying crypto library as it will restrain primitive
    // errors and we care only if recovery is passing or not.
    secp256k1::recover(&sig, hash.as_fixed_bytes()).ok()
}
```

In an [ECDSA (Elliptic Curve Digital Signature Algorithm) signature](https://wikipedia.org/wiki/Elliptic_Curve_Digital_Signature_Algorithm), the "r", "s", and "v" values are three pieces of data that are used to mathematically verify the authenticity of a digital signature. ECDSA is a widely used algorithm for generating and verifying digital signatures, and it is often used in cryptocurrencies like Ethereum.

The "r" is the x-coordinate of a point on the elliptic curve that is calculated as part of the signature process. The "s" is the s-value that is calculated during the signature process. It is derived from the private key and the message being signed. Lastly, the "v" is the "recovery value" that is used to recover the public key from the signature, which is derived from the signature and the message that was signed. Together, the "r", "s", and "v" values make up an ECDSA signature, and they are used to verify the authenticity of the signed transaction.

Once the transaction signer has been recovered, the signer is then added to the database. This process is repeated for every transaction that was retrieved, and similarly to previous stages, `Ok(ExecOutput { stage_progress, done: true })` is returned to signal a successful completion of the stage.

<br>

## ExecutionStage

Finally, after all headers, bodies and senders are added to the database, the `ExecutionStage` starts to execute. This stage is responsible for executing all of the transactions and updating the state stored in the database. For every new block header added to the database, the corresponding transactions have their signers attached to them and `reth_executor::executor::execute_and_verify_receipt()` is called, pushing the state changes resulting from the execution to a `Vec`.

[File: crates/stages/src/stages/execution.rs](https://github.com/paradigmxyz/reth/blob/main/crates/stages/src/stages/execution.rs)
```rust,ignore
pub fn execute_and_verify_receipt<DB: StateProvider>(
    block: &Block,
    total_difficulty: U256,
    senders: Option<Vec<Address>>,
    chain_spec: &ChainSpec,
    db: &mut SubState<DB>,
) -> Result<ExecutionResult, Error>
```

After all headers and their corresponding transactions have been executed, all of the resulting state changes are applied to the database, updating account balances, account bytecode and other state changes. After applying all of the execution state changes, if there was a block reward, it is applied to the validator's account. 

At the end of the `execute()` function, a familiar value is returned, `Ok(ExecOutput { stage_progress, done: true })` signaling a successful completion of the `ExecutionStage`.

<br>

## MerkleUnwindStage
* TODO: explain stage
<br>

## AccountHashingStage
* TODO: explain stage
<br>

## StorageHashingStage
* TODO: explain stage
<br>

## MerkleExecuteStage
* TODO: explain stage
<br>

## TransactionLookupStage
* TODO: explain stage
<br>

## IndexStorageHistoryStage
* TODO: explain stage
<br>

## IndexAccountHistoryStage
* TODO: explain stage
<br>

## FinishStage
* TODO: explain stage
<br>


# Next Chapter

Now that we have covered all of the stages that are currently included in the `Pipeline`, you know how the Reth client stays synced with the chain tip and updates the database with all of the new headers, bodies, senders and state changes. While this chapter provides an overview on how the pipeline stages work, the following chapters will dive deeper into the database, the networking stack and other exciting corners of the Reth codebase. Feel free to check out any parts of the codebase mentioned in this chapter, and when you are ready, the next chapter will dive into the `database`.

[Next Chapter]()
