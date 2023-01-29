# executor

If you recall during the execution stage of the Reth pipeline, new blocks that have been recently downloaded are executed and the resulting state changes are then added to the database. The `executor` crate contains the logic to execute blocks, handle state changes, verify block receipts and more. Let's take a look at how the crate works under the hood.

For context, lets look at where the `executor` crate is first called after starting the node. The code snippet below contains the `execute` method used during the `ExecutionStage` of the Reth pipeline. In this function, after newly downloaded blocks are fetched from the database the blocks are executed using `reth_executor::executor::execute_and_verify_receipt`, returning an `ExecutionResult`. 

[File: ]()
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

Let's take a closer look at the `execute_and_verify_receipt` function to understand how it works.

[File: ]()
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

This function breaks down into two major parts, execution and receipt verification. The function takes in a block header, a slice of signed transactions with the recovered signer's account attached, a slice of ommer block headers, the chain specification and a reference to a provider that enables access to cached state used during execution. In the first part of the function, the transactions contained in the block are executed, returning an [`ExecutionResult`](). The `ExecutionResult` is then used to verify the execution result receipt along with the header receipt root and logs bloom.  

The `execute` function is responsible for executing all of the transactions within the block, handling the execution results and returning a `TransactionChangeSet` that will later be committed to the database. 

At the beginning of the `execute` function, a new EVM environment is initialized using the `db`, `chain_spec` and `header` from the `execute_and_verify_receipt` function. Each transaction within the `transactions` slice is yielded from an iterator and executed with with EVM environment, returning an [`revm::ExecutionResult`](). 


[File: ]()
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

        // Fatal internal error.
        if exit_reason == revm::Return::FatalExternalError {
            return Err(Error::ExecutionFatalError)
        }

        let is_success = match exit_reason {
            revm::return_ok!() => true,
            revm::return_revert!() => false,
            _ => false,
            //e => return Err(Error::EVMError { error_code: e as u32 }),
        };

        // Add spend gas.
        cumulative_gas_used += gas_used;

        // Transform logs to reth format.
        let logs: Vec<Log> = logs
            .into_iter()
            .map(|l| Log {
                address: H160(l.address.0),
                topics: l.topics.into_iter().map(|h| H256(h.0)).collect(),
                data: l.data.into(),
            })
            .collect();

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
        })    }

    // --snip--

    if chain_spec.fork_block(Hardfork::Dao) == Some(header.number) {
        let mut irregular_state_changeset = dao_fork_changeset(db)?;
        irregular_state_changeset.extend(block_reward.take().unwrap_or_default().into_iter());
        block_reward = Some(irregular_state_changeset);
    }

    Ok(ExecutionResult { changesets, block_reward })
}
```


Following transaction execution the `exit_reason` is handled. The `exit_reason` is a [`Return`]() enum from the[`revm` crate]() representing the reason that the transaction exited. Transaction success variants include `Continue`, `Stop`, `Return`, `SelfDestruct` while transaction revert cases are `Revert`, `CallTooDeep`, and `OutOfFund`. The `exit_reason` is recorded as a success or failure, which is used later in the transaction change set's receipt. 

Next, the cumulative gas used for the block is incremented by the gas used by the transaction and the changes generated from the transaction execution are used to update the `db` state. A new `TransactionChangeSet` is generated, and added to a `Vec<TransactionChangeSet>`. If the block reward is still active (before Paris/Merge) an additional `TransactionChangeSet` is generated for the account that receives the reward.

Finally the `execution` function ends by returning an `ExecutionResult` containing the transaction changesets and an optional block reward.

Once the `execute` function returns, the `execute_and_verify_receipt` continues with verifying the receipt. 


[File: ]()
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

//TODO: describe the verify receipt function


Once the receipt has been successfully verified, the `execute_and_verify_receipt` completes, returning the `transaction_change_set` which is then handled in the next steps of the `ExecutionStage` of the Reth pipeline.