//! Real Ethereum execution controls for reset-independent corpus state.

use super::*;
use alloy_consensus::{proofs::calculate_receipt_root, Header, TxEip2930, TxReceipt};
use alloy_genesis::{Genesis, GenesisAccount};
use alloy_primitives::{bytes, TxKind};
use proptest::{prelude::*, test_runner::TestRunner};
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_db_common::init::init_genesis_with_settings;
use reth_ethereum_consensus::EthBeaconConsensus;
use reth_ethereum_primitives::{Block as EthBlock, BlockBody as EthBody, Transaction};
use reth_evm::execute::Executor;
use reth_evm_ethereum::EthEvmConfig;
use reth_primitives_traits::crypto::secp256k1::public_key_to_address;
use reth_provider::{
    providers::BlockchainProvider, test_utils::create_test_provider_factory_with_chain_spec,
    BlockWriter, ExecutionOutcome, HistoryWriter, LatestStateProvider, StateProvider,
    StateProviderFactory,
};
use reth_revm::{database::StateProviderDatabase, Database as _};
use reth_storage_api::StorageSettings;
use reth_testing_utils::generators::{generate_key, rng, sign_tx_with_key_pair};
use reth_trie_common::KeccakKeyHasher;

#[test]
fn deployment_zero_slot_survives_reset_and_missing_state_fails() -> eyre::Result<()> {
    for recreate in [false, true] {
        let key = generate_key(&mut rng());
        let sender = public_key_to_address(key.public_key());
        // Paris SELFDESTRUCT wipes both slots. CREATE2 recreates the same address
        // without writing storage; a later cold SLOAD must see zero, not old data.
        let deployer = Address::from([0x11; 20]);
        let runtime = bytes!("600054506001545036156010576000ff5b00");
        let init = bytes!("6012600c60003960126000f3600054506001545036156010576000ff5b00");
        let contract = if recreate {
            deployer.create2(B256::ZERO, alloy_primitives::keccak256(&init))
        } else {
            sender.create(0)
        };
        let mut alloc = BTreeMap::from([(
            sender,
            GenesisAccount { balance: U256::from(10u64).pow(U256::from(18)), ..Default::default() },
        )]);
        if recreate {
            let mut factory_code = bytes!("601e60126000396000601e60006000f55000").to_vec();
            factory_code.extend_from_slice(&init);
            alloc.insert(
                deployer,
                GenesisAccount { code: Some(factory_code.into()), ..Default::default() },
            );
            alloc.insert(
                contract,
                GenesisAccount {
                    nonce: Some(1),
                    code: Some(runtime),
                    storage: Some(BTreeMap::from([
                        (B256::ZERO, B256::from(U256::from(7))),
                        (B256::from(U256::from(1)), B256::from(U256::from(9))),
                    ])),
                    ..Default::default()
                },
            );
        }
        let chain = Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(Genesis { alloc, ..MAINNET.genesis.clone() })
                .paris_activated()
                .build(),
        );
        let factory = create_test_provider_factory_with_chain_spec(chain.clone());
        // The lightweight append helper does not apply hashed-storage wipes.
        // Use its supported plain-state layout for the destruction fixture.
        init_genesis_with_settings(
            &factory,
            if recreate { StorageSettings::v1() } else { StorageSettings::v2() },
        )?;
        let config = EthEvmConfig::ethereum(chain.clone());
        let consensus = EthBeaconConsensus::new(chain.clone());
        let mut blocks = Vec::new();
        let mut parent = chain.genesis_hash();
        let count = if recreate { 3 } else { 2 };
        for nonce in 0..count {
            let transaction = sign_tx_with_key_pair(
                key,
                Transaction::Eip2930(TxEip2930 {
                    chain_id: 1,
                    nonce,
                    gas_limit: 100_000,
                    gas_price: 1_500_000_000,
                    to: if recreate {
                        TxKind::Call(if nonce == 1 { deployer } else { contract })
                    } else if nonce == 0 {
                        TxKind::Create
                    } else {
                        TxKind::Call(contract)
                    },
                    // Deploy PUSH1 0; SLOAD; STOP, then read a never-written slot.
                    input: if recreate {
                        if nonce == 0 {
                            bytes!("01")
                        } else {
                            Default::default()
                        }
                    } else if nonce == 0 {
                        bytes!("6004600c60003960046000f360005400")
                    } else {
                        Default::default()
                    },
                    ..Default::default()
                }),
            );
            let mut block = EthBlock {
                header: Header {
                    number: nonce + 1,
                    parent_hash: parent,
                    gas_limit: 1_000_000,
                    base_fee_per_gas: Some(1_000_000_000),
                    ..Default::default()
                },
                body: EthBody { transactions: vec![transaction], ..Default::default() },
            };
            // Build the canonical fixture using the normal executor, before capture.
            let output = config
                .batch_executor(StateProviderDatabase::new(
                    LatestStateProvider::new(factory.provider()?).into_evm_state_provider(),
                ))
                .execute(&block.clone().try_into_recovered()?)?;
            assert!(output.receipts.iter().all(TxReceipt::status));
            block.header.gas_used = output.gas_used;
            block.header.receipts_root = calculate_receipt_root(
                &output.receipts.iter().map(TxReceipt::with_bloom_ref).collect::<Vec<_>>(),
            );
            let block = block.try_into_recovered()?;
            parent = block.hash();
            let mut outcome = ExecutionOutcome {
                bundle: output.state,
                receipts: vec![output.result.receipts],
                first_block: nonce + 1,
                requests: vec![output.result.requests],
            };
            outcome.bundle.reverts.sort();
            let hashed = outcome.hash_state_slow::<KeccakKeyHasher>().into_sorted();
            let writer = factory.provider_rw()?;
            writer.append_blocks_with_state(vec![block.clone()], &outcome, hashed)?;
            writer.commit()?;
            if recreate && nonce == 0 {
                // The test append helper indexes only explicit revm revert keys.
                // Like production history indexing, include DB-expanded wipe keys.
                let writer = factory.provider_rw()?;
                let transitions = writer
                    .storage_changeset(1)?
                    .into_iter()
                    .map(|(address, slot)| ((address.address(), slot.key), vec![1u64]))
                    .collect::<Vec<_>>();
                writer.insert_storage_history_index(transitions)?;
                writer.commit()?;
            }
            blocks.push(block);
        }
        let history = BlockchainProvider::new(factory.clone())?;
        if recreate {
            for (at, values) in [(0, [7, 9]), (1, [0, 0]), (2, [0, 0])] {
                let canonical = history.history_by_block_number(at)?;
                for (slot, value) in values.into_iter().enumerate() {
                    assert_eq!(
                        canonical
                            .storage(contract, B256::from(U256::from(slot)))?
                            .unwrap_or_default(),
                        U256::from(value),
                        "canonical at={at}, slot={slot}, account={:?}",
                        canonical.basic_account(&contract)?,
                    );
                }
                assert_eq!(canonical.basic_account(&contract)?.is_some(), at != 1);
            }
        }
        let expected = factory.provider()?;
        let token = CancellationToken::new();
        let captured = Chunk::capture(
            1..count + 1,
            &config,
            &consensus,
            &expected,
            |at| {
                Ok(StateProviderDatabase::new(
                    history.history_by_block_number(at)?.into_evm_state_provider(),
                ))
            },
            |at| Ok(blocks[(at - 1) as usize].clone()),
            &token,
        )?;
        let bytes = wincode::serialize(&captured)?;
        // Mutate real nested records, not predominantly rejected random prefixes.
        TestRunner::default()
            .run(&(0..bytes.len(), 1u8..=255), |(offset, mask)| {
                let mut mutated = bytes.clone();
                mutated[offset] ^= mask;
                if let Ok(decoded) = wincode::deserialize_exact::<Chunk>(&mutated) {
                    let encoded = wincode::serialize(&decoded).unwrap();
                    prop_assert!(wincode::deserialize_exact::<Chunk>(&encoded).is_ok());
                }
                Ok(())
            })
            .unwrap();
        for end in 0..bytes.len() {
            assert!(wincode::deserialize_exact::<Chunk>(&bytes[..end]).is_err());
        }
        let restored = Arc::new(wincode::deserialize::<Chunk>(&bytes)?);
        assert_eq!(restored.replay(&config, &consensus, &token)?.0, count);
        if recreate {
            for (at, values) in [(0, [7, 9]), (1, [0, 0]), (2, [0, 0])] {
                let mut view = View { chunk: restored.clone(), at };
                for (slot, value) in values.into_iter().enumerate() {
                    assert_eq!(view.storage(contract, U256::from(slot))?, U256::from(value));
                }
                assert_eq!(view.basic(contract)?.is_some(), at != 1);
            }
        }
        // Force a new State at each block, just as the shared executor does at its
        // size/time reset boundary, rather than relying on machine-dependent timing.
        let reset_replay = |chunk: &Arc<Chunk>, at| {
            super::super::execute::execute_chunk(
                &config,
                &consensus,
                chunk.as_ref(),
                at..at + 1,
                false,
                &token,
                |at| Ok(View { chunk: chunk.clone(), at }),
                |_| Ok(blocks[(at - 1) as usize].clone()),
                |_| Ok(Vec::new()),
                |_, _| {},
                |_, _| {},
            )
        };
        for at in 1..=count {
            reset_replay(&restored, at)?;
        }
        let mut missing: Chunk = wincode::deserialize(&bytes)?;
        assert!(missing.storage.remove(&(contract.into_array(), [0; 32])).is_some());
        assert!(reset_replay(&Arc::new(missing), count).is_err());
        let mut wrong_state: Chunk = wincode::deserialize(&bytes)?;
        wrong_state.blocks[0]
            .accounts
            .iter_mut()
            .find(|(address, _)| *address == sender.into_array())
            .unwrap()
            .1
            .as_mut()
            .unwrap()
            .nonce += 1;
        assert!(Arc::new(wrong_state).replay(&config, &consensus, &token).is_err());
        let mut wrong_receipt: Chunk = wincode::deserialize(&bytes)?;
        let last = wrong_receipt.blocks.last_mut().unwrap();
        let mut block: EthBlock = alloy_rlp::decode_exact(&last.rlp)?;
        block.header.receipts_root = B256::ZERO;
        last.hash = block.hash_slow().0;
        last.rlp = alloy_rlp::encode(block);
        assert!(Arc::new(wrong_receipt).replay(&config, &consensus, &token).is_err());
    }
    Ok(())
}
