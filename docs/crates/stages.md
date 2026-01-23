# Stages

The `stages` lib plays a central role in syncing the node, maintaining state, updating the database and more. The stages involved in the Reth pipeline are queued up and stored within the Reth pipeline. In the default configuration, the pipeline runs the following stages in order:

- EraStage (optional, for ERA1 import)
- HeaderStage
- BodyStage
- SenderRecoveryStage
- ExecutionStage
- PruneSenderRecoveryStage (if pruning for sender recovery is enabled)
- MerkleStage (unwind)
- AccountHashingStage
- StorageHashingStage
- MerkleStage (execute)
- TransactionLookupStage
- IndexStorageHistoryStage
- IndexAccountHistoryStage
- PruneStage
- FinishStage


When the node is first started, a new `Pipeline` is initialized and all of the stages are added into `Pipeline.stages`. Then, the `Pipeline::run` function is called, which starts the pipeline, executing all of the stages continuously in an infinite loop. This process syncs the chain, keeping everything up to date with the chain tip.

Each stage within the pipeline implements the `Stage` trait which provides function interfaces to get the stage id, execute the stage and unwind the changes to the database if there was an issue during the stage execution.

To get a better idea of what is happening at each part of the pipeline, let's walk through what is going on under the hood when a stage is executed, starting with `EraStage`.

<br>


## EraStage

The `EraStage` is an optional stage that imports pre-merge historical block data from [ERA1 files](https://github.com/eth-clients/e2store-format-specs/blob/main/formats/era1.md). ERA1 is a standardized format for storing Ethereum's historical chain data, allowing nodes to quickly bootstrap by importing pre-synced data instead of downloading it from peers.

When enabled, the `EraStage` reads block headers and bodies from ERA1 files (either from a local directory or downloaded from a remote HTTP host) and writes them directly to static files. This provides a faster alternative to downloading historical data over P2P, especially useful for syncing the pre-merge portion of the chain.

The stage processes ERA1 files sequentially, extracting headers and bodies from genesis up to the last pre-merge block. Note that receipts are not included in ERA1 files and will be generated later during the `ExecutionStage`.

If no ERA1 source is configured or all ERA1 data has been imported, the stage simply passes through, allowing subsequent stages to continue with P2P-based syncing.

<br>


## HeaderStage

The `HeaderStage` is responsible for syncing the block headers, validating the header integrity and writing the headers to storage. When the stage runs, it determines the sync gap between the local head and the tip, then downloads headers in reverse (from tip down to the local head) using a `HeaderDownloader` stream. Headers are buffered in ETL collectors and then written to static files and to `HeaderNumbers` in the database in a single step.

The `HeaderStage` relies on the downloader stream to return the headers in descending order starting from the chain tip down to the latest block in the database. While other stages in the `Pipeline` start from the most recent block in the database up to the chain tip, the `HeaderStage` works in reverse to avoid [long-range attacks](https://messari.io/report/long-range-attack). When a node downloads headers in ascending order, it will not know if it is being subjected to a long-range attack until it reaches the most recent blocks. To combat this, the `HeaderStage` starts by getting the chain tip, verifies the tip, and then walks backwards by the parent hash.

Each header is validated to ensure it correctly attaches to its parent and conforms to consensus expectations by the downloader before it is yielded. After download, headers are written to storage. If a header is not valid or the stream encounters any other error, the error is propagated up through the stage execution, the changes to the database are unwound and the stage is resumed from the most recent valid state.

This process continues until all of the headers have been downloaded and written to storage. Finally, the function returns, for example: `Ok(ExecOutput { checkpoint: StageCheckpoint::new(last_header_number).with_headers_stage_checkpoint(...), done: true })`, signaling that the header sync has been completed successfully.

<br>

## BodyStage

Once the `HeaderStage` completes successfully, the `BodyStage` will start execution. The body stage downloads block bodies for all of the new block headers that were stored locally in the database. The `BodyStage` first determines which block bodies to download by checking if the block body has an ommers hash and transaction root. 

An ommers hash is the Keccak 256-bit hash of the ommers list portion of the block. If you are unfamiliar with ommers blocks, you can [click here to learn more](https://ethereum.org/en/glossary/#ommer). Note that while ommers blocks were important for new blocks created during Ethereum's proof of work chain, Ethereum's proof of stake chain selects exactly one block proposer at a time, causing ommers blocks not to be needed in post-merge Ethereum.

The transactions root is a value that is calculated based on the transactions included in the block. To derive the transactions root, a [merkle tree](https://blog.ethereum.org/2015/11/15/merkling-in-ethereum) is created from the block's transactions list. The transactions root is then derived by taking the Keccak 256-bit hash of the root node of the merkle tree.

When the `BodyStage` is looking at the headers to determine which block to download, it will skip the blocks where the `header.ommers_hash` and the `header.transaction_root` are empty, denoting that the block is empty as well.

Once the `BodyStage` determines which block bodies to fetch, a new `bodies_stream` is created which downloads all of the bodies from the `starting_block`, up until the `target_block` is specified. Each time the `bodies_stream` yields a value, a response is received indicating either an empty block or a full block body to be written.

The `BodyStage` writes the received block bodies to storage. Validation of block body correctness relative to headers is enforced by the downloader and later by execution/consensus. This process is repeated for every downloaded block body, with the `BodyStage` returning `Ok(ExecOutput { checkpoint: StageCheckpoint::new(highest_block).with_entities_stage_checkpoint(...), done: ... })` signaling progress/completion.

<br>

## SenderRecoveryStage

Following a successful `BodyStage`, the `SenderRecoveryStage` starts to execute. The `SenderRecoveryStage` is responsible for recovering the transaction sender for each of the newly added transactions to the database. At the beginning of the execution function, all of the transactions are first retrieved from the database. Then the `SenderRecoveryStage` goes through each transaction and recovers the signer from the transaction signature and hash. The transaction hash is derived by taking the Keccak 256-bit hash of the RLP encoded transaction bytes. This hash is then passed into the `recover_signer` function.

In an [ECDSA (Elliptic Curve Digital Signature Algorithm) signature](https://wikipedia.org/wiki/Elliptic_Curve_Digital_Signature_Algorithm), the "r", "s", and "v" values are three pieces of data that are used to mathematically verify the authenticity of a digital signature. ECDSA is a widely used algorithm for generating and verifying digital signatures, and it is often used in cryptocurrencies like Ethereum.

The "r" is the x-coordinate of a point on the elliptic curve that is calculated as part of the signature process. The "s" is the s-value that is calculated during the signature process. It is derived from the private key and the message being signed. Lastly, the "v" is the "recovery value" that is used to recover the public key from the signature, which is derived from the signature and the message that was signed. Together, the "r", "s", and "v" values make up an ECDSA signature, and they are used to verify the authenticity of the signed transaction.

Once the transaction signer has been recovered, the signer is then added to the database. This process is repeated for every transaction that was retrieved, and similarly to previous stages, `Ok(ExecOutput { checkpoint: StageCheckpoint::new(end_block).with_entities_stage_checkpoint(...), done: ... })` is returned to signal a successful completion of the stage.

<br>

## ExecutionStage

Finally, after all headers, bodies and senders are added to the database, the `ExecutionStage` starts to execute. This stage is responsible for executing all of the transactions and updating the state stored in the database.

After all headers and their corresponding transactions have been executed, all of the resulting state changes are applied to the database, updating account balances, account bytecode and other state changes. In post-Merge Ethereum, there is no inflationary block reward on the Execution Layer; fees/priority tips are handled within transaction execution.

At the end of the `execute()` function, a familiar value is returned, `Ok(ExecOutput { checkpoint: StageCheckpoint::new(stage_progress).with_execution_stage_checkpoint(...), done: ... })` signaling a successful completion of the `ExecutionStage`.

<br>

## MerkleUnwindStage

The `MerkleUnwindStage` is responsible for unwinding the Merkle Patricia trie when reorgs occur or when there's a need to roll back state changes. This ensures the trie remains consistent with the chain's canonical history by reverting changes beyond the unwind point. It typically runs before the hashing stages to unwind trie state during reorgs or rollbacks.

## MerkleExecuteStage

The `MerkleExecuteStage` runs after `AccountHashingStage` and `StorageHashingStage` and is responsible for constructing or updating the state root based on the latest hashed account and storage data. It processes state changes from executed transactions and maintains the state root included in block headers.

<br>

## AccountHashingStage

The `AccountHashingStage` handles the computation of account state hashes. It processes all accounts in the state and computes their cryptographic hashes, which are essential for building the state trie. This stage is crucial for maintaining the integrity of the state and enabling efficient state proof verification.

<br>

## StorageHashingStage

The `StorageHashingStage` is responsible for computing hashes of contract storage. Similar to the `AccountHashingStage`, it processes storage slots of smart contracts and generates cryptographic hashes that are used in the state trie. This stage ensures that contract storage can be efficiently verified and proven.

<br>

## TransactionLookupStage

The `TransactionLookupStage` builds and maintains transaction lookup indices. These indices enable efficient querying of transactions by hash or block position. This stage is crucial for RPC functionality, allowing users to quickly retrieve transaction information without scanning the entire blockchain.

<br>

## IndexStorageHistoryStage

The `IndexStorageHistoryStage` creates indices for historical contract storage states. It tracks how contract storage values change over time, enabling historical state queries. This is essential for features like state debugging, transaction tracing, and historical state access.

<br>

## IndexAccountHistoryStage

The `IndexAccountHistoryStage` builds indices for account history, tracking how account states (balance, nonce, code) change over time. Similar to the storage history stage, this enables historical queries of account states at any block height, which is crucial for debugging and analysis tools.

<br>

## PruneSenderRecoveryStage

The `PruneSenderRecoveryStage` removes entries from `TransactionSenders` according to configured prune modes. It typically runs after `ExecutionStage` when pruning for sender recovery is enabled.

<br>

## PruneStage

The `PruneStage` performs pruning for the configured segments (such as history tables) based on `PruneModes`. It runs after hashing/merkle and history indexing stages.

<br>

## FinishStage

The `FinishStage` is the final stage in the pipeline that performs cleanup and verification tasks. It ensures that all previous stages have been completed successfully and that the node's state is consistent. This stage may also update various metrics and status indicators to reflect the completion of a sync cycle.

<br>

# Next Chapter

Now that we have covered all of the stages that are currently included in the `Pipeline`, you know how the Reth client stays synced with the chain tip and updates the database with all of the new headers, bodies, senders and state changes. While this chapter provides an overview on how the pipeline stages work, the following chapters will dive deeper into the database, the networking stack and other exciting corners of the Reth codebase. Feel free to check out any parts of the codebase mentioned in this chapter, and when you are ready, the next chapter will dive into the `database`.

[Next Chapter](db.md)
