# executor

The `executor` crate contains the logic to execute blocks, handle account state changes, verify block receipts and more. Let's take a look at how the crate works under the hood.

For context, after starting the node, the `executor` crate is first called during the execution stage of the Reth pipeline. Within the `ExecutionStage::execute` function, after newly downloaded blocks are fetched from the database, the blocks are executed using `reth_executor::executor::execute_and_verify_receipt`, returning an `ExecutionResult`. 

[File: crates/stages/src/stages/execution.rs](https://github.com/paradigmxyz/reth/blob/main/crates/stages/src/stages/execution.rs#L82)
```rust ignore
#[async_trait::async_trait]
impl<DB: Database> Stage<DB> for ExecutionStage {
    //--snip--

    /// Execute the stage
    async fn execute(
        &mut self,
        tx: &mut Transaction<'_, DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        //--snip--

        // Fetch transactions, execute them and generate results
        let mut block_change_patches = Vec::with_capacity(canonical_batch.len());

        for (header, body, ommers) in block_batch.iter() {
          
            let changeset = std::thread::scope(|scope| {
                let handle = std::thread::Builder::new()
                    .stack_size(50 * 1024 * 1024)
                    .spawn_scoped(scope, || {
                        // execute and store output to results
                        reth_executor::executor::execute_and_verify_receipt(
                            header,
                            &recovered_transactions,
                            ommers,
                            &self.chain_spec,
                            &mut state_provider,
                        )
                    })
                    .expect("Expects that thread name is not null");
                handle.join().expect("Expects for thread to not panic")
            })
            .map_err(|error| StageError::ExecutionError { block: header.number, error })?;
            block_change_patches.push(changeset);
        }
        //--snip--
    }
}
```

As the function name suggests, the `execute_and_verify_receipt` function breaks down into two major parts, execution and receipt verification.

[File: crates/executor/src/executor.rs](https://github.com/paradigmxyz/reth/blob/main/crates/executor/src/executor.rs#L245)
```rust ignore
/// Execute and verify block
pub fn execute_and_verify_receipt<DB: StateProvider>(
    header: &Header,
    transactions: &[TransactionSignedEcRecovered],
    ommers: &[Header],
    chain_spec: &ChainSpec,
    db: &mut SubState<DB>,
) -> Result<ExecutionResult, Error> {
    let transaction_change_set = execute(header, transactions, ommers, chain_spec, db)?;

    let receipts_iter =
        transaction_change_set.changesets.iter().map(|changeset| &changeset.receipt);

    if Some(header.number) >= chain_spec.fork_block(Hardfork::Byzantium) {
        verify_receipt(header.receipts_root, header.logs_bloom, receipts_iter)?;
    }

    Ok(transaction_change_set)
}
```

The `execute` function is responsible for executing all of the transactions within the block, handling the execution results and returning a `TransactionChangeSet` that will later be committed to the database. 

At the beginning of the `execute` function, a new EVM environment is initialized using the `db`, `chain_spec` and `header` passed in from the `execute_and_verify_receipt` function. Each transaction within the `transactions` slice is yielded from an iterator and executed with the recently initialized EVM environment, returning a [`revm::ExecutionResult`](https://github.com/bluealloy/revm/blob/main/crates/primitives/src/result.rs#L17). 

[File: crates/executor/src/executor.rs](https://github.com/paradigmxyz/reth/blob/main/crates/executor/src/executor.rs#L295)
```rust ignore
pub fn execute<DB: StateProvider>(
    header: &Header,
    transactions: &[TransactionSignedEcRecovered],
    ommers: &[Header],
    chain_spec: &ChainSpec,
    db: &mut SubState<DB>,
) -> Result<ExecutionResult, Error> {
    let mut evm = EVM::new();
    evm.database(db);

    let spec_id = revm_spec(chain_spec, header.number);
    evm.env.cfg.chain_id = U256::from(chain_spec.chain().id());
    evm.env.cfg.spec_id = spec_id;
    evm.env.cfg.perf_all_precompiles_have_balance = false;
    evm.env.cfg.perf_analyse_created_bytecodes = AnalysisKind::Raw;

    revm_wrap::fill_block_env(&mut evm.env.block, header, spec_id >= SpecId::MERGE);
    let mut cumulative_gas_used = 0;
    // output of verification
    let mut changesets = Vec::with_capacity(transactions.len());

    for transaction in transactions.iter() {
        // The sum of the transaction’s gas limit, Tg, and the gas utilised in this block prior,
        // must be no greater than the block’s gasLimit.
        let block_available_gas = header.gas_limit - cumulative_gas_used;
        if transaction.gas_limit() > block_available_gas {
            return Err(Error::TransactionGasLimitMoreThenAvailableBlockGas {
                transaction_gas_limit: transaction.gas_limit(),
                block_available_gas,
            })
        }

        // Fill revm structure.
        revm_wrap::fill_tx_env(&mut evm.env.tx, transaction);

        // Execute transaction.
        let out = evm.transact();


        let (revm::ExecutionResult { exit_reason, gas_used, logs, .. }, state) = out;

        //--snip--

        // commit state
        let (changeset, new_bytecodes) =
            commit_changes(evm.db().expect("Db to not be moved."), state);

        }
}
```


Following transaction execution, the `exit_reason` is handled. The `exit_reason` is a `revm::Return` enum, representing the reason that the transaction exited. Transaction success variants include `Continue`, `Stop`, `Return` and `SelfDestruct` while transaction revert cases include `Revert`, `CallTooDeep`, and `OutOfFund`. The `exit_reason` is recorded as a success or failure, which is used later in the `TransactionChangeSet.receipt`. 

Next, the cumulative gas used for the block is incremented and any account changes generated from the transaction execution are updated within the `db`. The `commit_changes` function generates an `AccountChangeSet` for each account where code is created, destroyed or changed.  

[File: crates/executor/src/executor.rs](https://github.com/paradigmxyz/reth/blob/main/crates/executor/src/executor.rs#L90)
```rust ignore
/// Diff change set that is needed for creating history index and updating current world state.
#[derive(Debug, Clone)]
pub struct AccountChangeSet {
    /// Old and New account account change.
    pub account: AccountInfoChangeSet,
    /// Storage containing key -> (OldValue,NewValue). in case that old value is not existing
    /// we can expect to have U256::ZERO, same with new value.
    pub storage: BTreeMap<U256, (U256, U256)>,
    /// Just to make sure that we are taking selfdestruct cleaning we have this field that wipes
    /// storage. There are instances where storage is changed but account is not touched, so we
    /// can't take into account that if new account is None that it is selfdestruct.
    pub wipe_storage: bool,
}
```

A new `TransactionChangeSet` is generated and if the block reward is still active (before Paris/Merge) an additional `TransactionChangeSet` is generated for the account that receives the reward. Finally the `execution` function ends by returning an `ExecutionResult` containing the transaction changesets and an optional block reward.


[File: crates/executor/src/executor.rs](https://github.com/paradigmxyz/reth/blob/main/crates/executor/src/executor.rs#L369)

```rust ignore
pub fn execute<DB: StateProvider>(
    header: &Header,
    transactions: &[TransactionSignedEcRecovered],
    ommers: &[Header],
    chain_spec: &ChainSpec,
    db: &mut SubState<DB>,
) -> Result<ExecutionResult, Error> {
    //--snip--
    for transaction in transactions.iter() {
        //--snip--

        // commit state
        let (changeset, new_bytecodes) =
            commit_changes(evm.db().expect("Db to not be moved."), state);

        // Push transaction changeset and calculte header bloom filter for receipt.
        changesets.push(TransactionChangeSet {
            receipt: Receipt {
                tx_type: transaction.tx_type(),
                success: is_success,
                cumulative_gas_used,
                bloom: logs_bloom(logs.iter()),
                logs,
            },
            changeset,
            new_bytecodes,
        })    
    }

    // --snip--

    if chain_spec.fork_block(Hardfork::Dao) == Some(header.number) {
        let mut irregular_state_changeset = dao_fork_changeset(db)?;
        irregular_state_changeset.extend(block_reward.take().unwrap_or_default().into_iter());
        block_reward = Some(irregular_state_changeset);
    }

    Ok(ExecutionResult { changesets, block_reward })
}
```


Once the `execute` function returns, the `execute_and_verify_receipt` functions continues with verifying the receipt by calculating the `receipts_root` and comparing it to the `header.receipts_root` for the block that is being verified. In addition to the `receipts_root`, the `logs_bloom` is also evaluated against the `header.logs_bloom`.

[File: crates/executor/src/executor.rs](https://github.com/paradigmxyz/reth/blob/main/crates/executor/src/executor.rs#L269)
```rust ignore
/// Verify receipts
pub fn verify_receipt<'a>(
    expected_receipts_root: H256,
    expected_logs_bloom: Bloom,
    receipts: impl Iterator<Item = &'a Receipt> + Clone,
) -> Result<(), Error> {
    // Check receipts root.
    let receipts_root = reth_primitives::proofs::calculate_receipt_root(receipts.clone());
    if receipts_root != expected_receipts_root {
        return Err(Error::ReceiptRootDiff { got: receipts_root, expected: expected_receipts_root })
    }

    // Create header log bloom.
    let logs_bloom = receipts.fold(Bloom::zero(), |bloom, r| bloom | r.bloom);
    if logs_bloom != expected_logs_bloom {
        return Err(Error::BloomLogDiff {
            expected: Box::new(expected_logs_bloom),
            got: Box::new(logs_bloom),
        })
    }
    Ok(())
}
```


After the receipt has been successfully verified, the `execute_and_verify_receipt` completes, returning the `transaction_change_set` which is then handled in the next steps of the `ExecutionStage` of the Reth pipeline.