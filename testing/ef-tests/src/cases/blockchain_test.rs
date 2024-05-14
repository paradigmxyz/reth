//! Test runners for `BlockchainTests` in <https://github.com/ethereum/tests>

use crate::{
    case::CaseAsync,
    models::{BlockchainTest, ForkSpec, State},
    suite::SuiteAsync,
    Error,
};
use alloy_rlp::{Decodable, Encodable};
use anyhow::{bail, Result};
use raiko_host::{
    error::{HostError, HostResult},
    raiko::{BlockDataProvider, NativeResponse, Raiko},
    request::{ProofRequest, ProofType},
    MerkleProof,
};
use raiko_lib::{
    consts::{ChainSpec, Eip1559Constants, Network},
    input::GuestOutput,
};
use raiko_primitives::{
    keccak::keccak,
    mpt::{to_nibs, MptNode, MptNodeData, StateAccount},
};
use reth_db::{
    mdbx::{tx::Tx, RO},
    test_utils::{create_test_rw_db, create_test_static_files_dir},
};
use reth_node_ethereum::EthEvmConfig;
use reth_primitives::{
    address, b256, stage::StageCheckpoint, Address, BlockBody, SealedBlock, StaticFileSegment,
    B256, U256,
};
use reth_provider::{
    providers::StaticFileWriter, AccountReader, BlockReader, DatabaseProvider, ProviderFactory,
    StateProvider,
};
use reth_revm::primitives::{AccountInfo, SpecId};
use reth_rpc_types::{
    Block as RpcBlock, BlockTransactions, EIP1186AccountProofResponse, EIP1186StorageProof,
};
use reth_rpc_types_compat::{
    block::from_block_with_transactions, proof::from_primitive_account_proof,
    transaction::from_recovered_with_block_context,
};
use reth_stages::{
    stages::{AccountHashingStage, ExecutionStage, MerkleStage, StorageHashingStage},
    ExecInput, Stage,
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs,
    path::Path,
    sync::Arc,
};

/// A handler for the blockchain test suite.
#[derive(Debug)]
pub struct BlockchainTests {
    suite: String,
}

impl BlockchainTests {
    /// Create a new handler for a subset of the blockchain test suite.
    pub fn new(suite: String) -> Self {
        Self { suite }
    }
}

impl SuiteAsync for BlockchainTests {
    type CaseAsync = BlockchainTestCase;

    fn suite_name(&self) -> String {
        format!("BlockchainTests/{}", self.suite)
    }
}

/// An Ethereum blockchain test.
#[derive(Debug, PartialEq, Eq)]
pub struct BlockchainTestCase {
    name: String,
    tests: BTreeMap<String, BlockchainTest>,
    skip: bool,
}

/// Computes the Merkle proof for the given key in the trie.
pub fn mpt_proof(root: &MptNode, key: impl AsRef<[u8]>) -> Result<Vec<Vec<u8>>, anyhow::Error> {
    let mut path = proof_internal(root, &to_nibs(key.as_ref()))?;
    path.reverse();
    Ok(path)
}

fn proof_internal(node: &MptNode, key_nibs: &[u8]) -> Result<Vec<Vec<u8>>, anyhow::Error> {
    if key_nibs.is_empty() {
        return Ok(vec![alloy_rlp::encode(node)]);
    }

    let mut path: Vec<Vec<u8>> = match node.as_data() {
        MptNodeData::Null | MptNodeData::Leaf(_, _) => {
            println!("test leaf/null");
            vec![]
        }
        MptNodeData::Branch(children) => {
            println!("test Branch");
            let (i, tail) = key_nibs.split_first().unwrap();
            match &children[*i as usize] {
                Some(child) => proof_internal(child, tail)?,
                None => vec![],
            }
        }
        MptNodeData::Extension(_, child) => {
            println!("test Extension");
            if let Some(tail) = key_nibs.strip_prefix(node.nibs().as_slice()) {
                proof_internal(child, tail)?
            } else {
                vec![]
            }
        }
        MptNodeData::Digest(_) => bail!("Cannot descend pointer!"),
    };
    println!("Adding: {:?}", alloy_rlp::encode(node));
    path.push(alloy_rlp::encode(node));

    Ok(path)
}

/// Builds the state trie and storage tries from the test state.
pub fn build_tries(state: &State) -> (MptNode, HashMap<Address, MptNode>) {
    let mut state_trie = MptNode::default();
    let mut storage_tries = HashMap::new();
    for (address, account) in state.iter() {
        let mut storage_trie = MptNode::default();
        for (slot, value) in &account.storage {
            if *value != U256::ZERO {
                storage_trie.insert_rlp(&keccak(slot.to_be_bytes::<32>()), *value).unwrap();
            }
        }

        state_trie
            .insert_rlp(
                &keccak(address),
                StateAccount {
                    nonce: account.nonce.try_into().unwrap(),
                    balance: account.balance,
                    storage_root: storage_trie.hash(),
                    code_hash: keccak(account.code.clone()).into(),
                },
            )
            .unwrap();
        storage_tries.insert(*address, storage_trie);
    }

    println!("test state trie root: {:?}", state_trie.hash());

    (state_trie, storage_tries)
}

fn get_proof(
    address: Address,
    indices: impl IntoIterator<Item = U256>,
    state: &State,
) -> Result<EIP1186AccountProofResponse, anyhow::Error> {
    let account = state.get(&address).cloned().unwrap_or_default();
    let (state_trie, mut storage_tries) = build_tries(state);
    let storage_trie = storage_tries.remove(&address).unwrap_or_default();

    let account_proof =
        mpt_proof(&state_trie, keccak(address))?.into_iter().map(|p| p.into()).collect();
    let mut storage_proof = vec![];
    let index_set = indices.into_iter().collect::<BTreeSet<_>>();
    for index in index_set {
        let proof = EIP1186StorageProof {
            key: index.into(),
            proof: mpt_proof(&storage_trie, keccak(index.to_be_bytes::<32>()))?
                .into_iter()
                .map(|p| p.into())
                .collect(),
            value: account.storage.get(&index).cloned().unwrap_or_default(),
        };
        storage_proof.push(proof);
    }

    Ok(EIP1186AccountProofResponse {
        address: address.into_array().into(),
        balance: account.balance,
        code_hash: keccak(account.code).into(),
        nonce: account.nonce.into_limbs()[0].try_into().unwrap(),
        storage_hash: storage_trie.hash().0.into(),
        account_proof,
        storage_proof,
    })
}

fn remove_duplicates(objects: Vec<EIP1186StorageProof>) -> Vec<EIP1186StorageProof> {
    let mut unique_keys = HashSet::new();
    let mut unique_objects = Vec::new();

    for object in objects {
        if unique_keys.insert(object.key.0) {
            unique_objects.push(object);
        }
    }

    unique_objects
}

/// RethBlockDataProvider
pub struct RethBlockDataProvider {
    provider: DatabaseProvider<Tx<RO>>,
    block_number: u64,
    block_state_pre: Box<dyn StateProvider>,
    block_state_post: Box<dyn StateProvider>,
    test_state_pre: State,
    test_state_post: State,
}

/// RethBlockDataProvider impl
impl RethBlockDataProvider {
    /// Creates a new RethBlockDataProvider
    pub fn new(
        provider: DatabaseProvider<Tx<RO>>,
        block_number: u64,
        block_state_pre: Box<dyn StateProvider>,
        block_state_post: Box<dyn StateProvider>,
        test_state_pre: State,
        test_state_post: State,
    ) -> Self {
        Self {
            provider,
            block_number,
            block_state_pre,
            block_state_post,
            test_state_pre,
            test_state_post,
        }
    }
}

/// Root hash of an empty trie.
pub const EMPTY_ROOT: B256 =
    b256!("56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421");

/// Raiko interface implementation for block data to directly connect with reth's database
impl BlockDataProvider for RethBlockDataProvider {
    async fn get_blocks(&self, blocks_to_fetch: &[(u64, bool)]) -> HostResult<Vec<RpcBlock>> {
        let mut blocks = Vec::new();

        for (block_number, _full) in blocks_to_fetch {
            let block = self
                .provider
                .block_by_number(*block_number)
                .expect("failed to fetch block")
                .unwrap();
            //println!("block: {:?}", block.unwrap().body);

            let senders = block.senders().unwrap();
            let block = block.try_with_senders_unchecked(senders).unwrap();

            let block_hash = block.header.hash_slow();
            let block_number = block.number;
            let base_fee_per_gas = Some(block.base_fee_per_gas.unwrap_or(1));

            let body = &block.body;
            let transactions_with_senders = body.into_iter().zip(block.senders.clone());
            let transactions = transactions_with_senders
                .enumerate()
                .map(|(idx, (tx, sender))| {
                    let signed_tx_ec_recovered = tx.clone().with_signer(sender);

                    from_recovered_with_block_context(
                        signed_tx_ec_recovered,
                        block_hash,
                        block_number,
                        base_fee_per_gas,
                        idx,
                    )
                })
                .collect::<Vec<_>>();

            let total_difficulty = U256::ZERO;
            let block = from_block_with_transactions(
                block.length(),
                block_hash,
                block.block,
                total_difficulty,
                BlockTransactions::Full(transactions),
            );
            blocks.push(block);
        }
        Ok(blocks)
    }

    async fn get_storage_values(&self, accounts: &[(Address, U256)]) -> HostResult<Vec<U256>> {
        let mut values = Vec::new();
        for (address, key) in accounts {
            let value = self.block_state_pre.storage(*address, (*key).into());
            let value = value.unwrap().or_else(|| Some(U256::ZERO)).unwrap();
            values.push(value);
        }
        Ok(values)
    }

    async fn get_accounts(&self, addresses: &[Address]) -> HostResult<Vec<AccountInfo>> {
        let mut accounts = Vec::new();
        for address in addresses {
            let account = self
                .block_state_pre
                .basic_account(*address)
                .map_err(|e| HostError::Anyhow(e.into()))?;
            //println!("bdp account {address}: {:?}", account);
            let code = self.block_state_pre.account_code(*address).expect("code failed");
            //println!("bdp account {address}: {:?}", account);
            let account = if let Some(account) = account {
                AccountInfo {
                    balance: account.balance,
                    nonce: account.nonce,
                    code_hash: account.get_bytecode_hash(),
                    code: code.map(|code| {
                        reth_primitives::revm_primitives::Bytecode::new_raw(code.bytecode.clone())
                    }),
                }
            } else {
                AccountInfo::default()
            };
            accounts.push(account);
        }
        Ok(accounts)
    }

    async fn get_merkle_proofs(
        &self,
        block_number: u64,
        accounts: HashMap<Address, Vec<U256>>,
        _offset: usize,
        _num_storage_proofs: usize,
    ) -> HostResult<MerkleProof> {
        let mut storage_proofs: MerkleProof = HashMap::new();

        println!("block_number: {block_number} ({})", self.block_number);

        let block_state = if block_number == self.block_number {
            &self.block_state_pre
        } else if block_number == self.block_number + 1 {
            &self.block_state_post
        } else {
            unreachable!()
        };

        let block_state_test = if block_number == self.block_number {
            &self.test_state_pre
        } else if block_number == self.block_number + 1 {
            &self.test_state_post
        } else {
            unreachable!()
        };

        /*for (account, keys) in accounts.iter() {
            let keys = keys.into_iter().map(|v| v.clone().into()).collect::<Vec<_>>();
            let proof = block_state.as_ref().proof(*account, &keys).unwrap();
            let prim_proof = from_primitive_account_proof(proof);
            println!("-- {:?}: {:?}", account, prim_proof);
            storage_proofs.insert(*account, prim_proof);
        }*/

        for (address, keys) in accounts.iter() {
            let indices = keys.into_iter().map(|v| v.clone().into()).collect::<Vec<_>>();
            let prim_proof_test = get_proof(*address, indices, &block_state_test)?;

            let indices = keys.into_iter().map(|v| v.clone().into()).collect::<Vec<_>>();
            let proof = block_state.as_ref().proof(*address, &indices).unwrap();
            let mut prim_proof = from_primitive_account_proof(proof);

            for storage_proof in prim_proof.storage_proof.iter_mut() {
                if storage_proof.proof.is_empty() {
                    storage_proof.proof.push(vec![128u8].into());
                }
            }

            prim_proof.storage_proof = remove_duplicates(prim_proof.storage_proof);

            assert_eq!(prim_proof_test.address, prim_proof.address);
            assert_eq!(prim_proof_test.balance, prim_proof.balance);
            assert_eq!(prim_proof_test.code_hash, prim_proof.code_hash);
            assert_eq!(prim_proof_test.nonce, prim_proof.nonce);
            let storage_hash = if prim_proof.storage_hash == B256::ZERO {
                EMPTY_ROOT
            } else {
                prim_proof.storage_hash
            };
            assert_eq!(prim_proof_test.storage_hash, storage_hash);
            /*if prim_proof_test.account_proof != prim_proof.account_proof {
                prim_proof.account_proof = prim_proof_test.account_proof.clone();
            }*/
            let account = self
                .block_state_pre
                .basic_account(*address)
                .map_err(|e| HostError::Anyhow(e.into()))?;
            if account.is_some() {
                assert_eq!(prim_proof_test.account_proof, prim_proof.account_proof);
            }
            assert_eq!(prim_proof_test.storage_proof.len(), prim_proof.storage_proof.len());
            /*for storage_proof in prim_proof.storage_proof.iter() {
                let value = self.block_state_pre.storage(*address, storage_proof.key.0);
                if value.is_ok() {
                    if value.unwrap().is_some() {
                        assert!(prim_proof_test.storage_proof.contains(storage_proof));
                    }
                }
            }*/

            //let proof = block_state..proof(*account, &keys).unwrap();
            //let prim_proof = from_primitive_account_proof(proof);
            //println!("-- {:?}: {:?}", account, prim_proof);
            //storage_proofs.insert(*account, prim_proof);
            storage_proofs.insert(*address, prim_proof_test);
        }

        Ok(storage_proofs)
    }
}

impl CaseAsync for BlockchainTestCase {
    fn load(path: &Path) -> Result<Self, Error> {
        Ok(BlockchainTestCase {
            name: path.to_str().unwrap().to_string(),
            tests: {
                let s = fs::read_to_string(path)
                    .map_err(|error| Error::Io { path: path.into(), error })?;
                serde_json::from_str(&s)
                    .map_err(|error| Error::CouldNotDeserialize { path: path.into(), error })?
            },
            skip: should_skip(path),
        })
    }

    /// Runs the test cases for the Ethereum Forks test suite.
    ///
    /// # Errors
    /// Returns an error if the test is flagged for skipping or encounters issues during execution.
    async fn run(&self) -> Result<(), Error> {
        // If the test is marked for skipping, return a Skipped error immediately.
        if self.skip {
            return Err(Error::Skipped)
        }

        // Iterate through test cases, filtering by the network type to exclude specific forks.
        let tests = self
            .tests
            .iter()
            .filter(|(name, case)| {
                // !matches!(
                //     case.network,
                //     ForkSpec::ByzantiumToConstantinopleAt5 |
                //         ForkSpec::Constantinople |
                //         ForkSpec::ConstantinopleFix |
                //         ForkSpec::MergeEOF |
                //         ForkSpec::MergeMeterInitCode |
                //         ForkSpec::MergePush0 |
                //         ForkSpec::Unknown
                matches!(case.network, ForkSpec::Shanghai) /* && !(name.starts_with("push0") ||
                                                            * name.starts_with("
                                                            * create2InitCodeSizeLimit")) */
            })
            .collect::<Vec<_>>();

        for (name, case) in tests.iter() {
            println!("## case: {}", name);
            // Create a new test database and initialize a provider for the test case.
            let db = create_test_rw_db();
            let (_static_files_dir, static_files_dir_path) = create_test_static_files_dir();
            let provider = ProviderFactory::new(
                db.as_ref(),
                Arc::new(case.network.clone().into()),
                static_files_dir_path.clone(),
            )?
            .provider_rw()
            .unwrap();

            //let eth_api = build_test_eth_api(provider);

            // Insert initial test state into the provider.
            provider
                .insert_historical_block(
                    SealedBlock::new(
                        case.genesis_block_header.clone().into(),
                        BlockBody::default(),
                    )
                    .try_seal_with_senders()
                    .unwrap(),
                    None,
                )
                .map_err(|err| Error::RethError(err.into()))?;
            case.pre.write_to_db(provider.tx_ref())?;

            // Initialize receipts static file with genesis
            {
                let mut receipts_writer = provider
                    .static_file_provider()
                    .latest_writer(StaticFileSegment::Receipts)
                    .unwrap();
                receipts_writer.increment_block(StaticFileSegment::Receipts, 0).unwrap();
                receipts_writer.commit_without_sync_all().unwrap();
            }

            // Decode and insert blocks, creating a chain of blocks for the test case.
            let last_block = case.blocks.iter().try_fold(None, |_, block| {
                let decoded = SealedBlock::decode(&mut block.rlp.as_ref())?;
                provider
                    .insert_historical_block(decoded.clone().try_seal_with_senders().unwrap(), None)
                    .map_err(|err| Error::RethError(err.into()))?;
                Ok::<Option<SealedBlock>, Error>(Some(decoded))
            })?;

            provider
                .static_file_provider()
                .latest_writer(StaticFileSegment::Headers)
                .unwrap()
                .commit()
                .unwrap();

            // Look up merkle checkpoint
            /*let merkle_checkpoint = provider
                .get_stage_checkpoint(StageId::MerkleExecute).unwrap()
                .expect("merkle checkpoint exists");

            let merkle_block_number = merkle_checkpoint.block_number;
            println!("merkle_block_number: {}", merkle_block_number);*/

            let mut account_hashing_stage = AccountHashingStage::default();
            let mut storage_hashing_stage = StorageHashingStage::default();
            let mut merkle_stage = MerkleStage::default_execution();

            let progress = None;

            let block = 0;

            let mut account_hashing_done = false;
            while !account_hashing_done {
                let output = account_hashing_stage
                    .execute(
                        &provider,
                        ExecInput {
                            target: Some(block),
                            checkpoint: progress.map(StageCheckpoint::new),
                        },
                    )
                    .unwrap();
                account_hashing_done = output.done;
            }

            let mut storage_hashing_done = false;
            while !storage_hashing_done {
                let output = storage_hashing_stage
                    .execute(
                        &provider,
                        ExecInput {
                            target: Some(block),
                            checkpoint: progress.map(StageCheckpoint::new),
                        },
                    )
                    .unwrap();
                storage_hashing_done = output.done;
            }

            let incremental_result = merkle_stage.execute(
                &provider,
                ExecInput { target: Some(block), checkpoint: progress.map(StageCheckpoint::new) },
            );
            println!("{:?}", incremental_result);
            assert!(incremental_result.is_ok());

            provider.commit().unwrap();

            let block_state_pre = ProviderFactory::new(
                db.as_ref(),
                Arc::new(case.network.clone().into()),
                static_files_dir_path.clone(),
            )?
            .latest()
            .unwrap();

            let coinbase = address!("2adc25665018aa1fe0e6bc666dac8fc2697ff9ba");
            let proof = block_state_pre.as_ref().proof(coinbase, &Vec::new()).unwrap();
            let prim_proof = from_primitive_account_proof(proof);
            println!("prim_proof: {:?}", prim_proof);

            //println!("account test: {:?}",
            // block_state_pre.basic_account(address!("a94f5374Fce5edBC8E2a8697C15331677e6EbF0B")));

            let provider = ProviderFactory::new(
                db.as_ref(),
                Arc::new(case.network.clone().into()),
                static_files_dir_path.clone(),
            )?
            .provider_rw()
            .unwrap();

            // Execute the execution stage using the EVM processor factory for the test case
            // network.
            let mut executor =
                ExecutionStage::new_with_factory(reth_revm::EvmProcessorFactory::new(
                    Arc::new(case.network.clone().into()),
                    EthEvmConfig::default(),
                ));

            let execution_result = executor.execute(
                &provider,
                ExecInput { target: last_block.as_ref().map(|b| b.number), checkpoint: None },
            );
            println!("execution result: {:?}", execution_result);

            let mut account_hashing_stage = AccountHashingStage::default();
            let mut storage_hashing_stage = StorageHashingStage::default();
            let mut merkle_stage = MerkleStage::default_execution();

            let progress = None;

            let block = 1;

            let mut account_hashing_done = false;
            while !account_hashing_done {
                let output = account_hashing_stage
                    .execute(
                        &provider,
                        ExecInput {
                            target: Some(block),
                            checkpoint: progress.map(StageCheckpoint::new),
                        },
                    )
                    .unwrap();
                account_hashing_done = output.done;
            }

            let mut storage_hashing_done = false;
            while !storage_hashing_done {
                let output = storage_hashing_stage
                    .execute(
                        &provider,
                        ExecInput {
                            target: Some(block),
                            checkpoint: progress.map(StageCheckpoint::new),
                        },
                    )
                    .unwrap();
                storage_hashing_done = output.done;
            }

            let incremental_result = merkle_stage.execute(
                &provider,
                ExecInput { target: Some(block), checkpoint: progress.map(StageCheckpoint::new) },
            );
            println!("{:?}", incremental_result);
            //assert!(incremental_result.is_ok());
            //if execution_result.is_ok() {
            //assert!(incremental_result.is_ok());
            //}

            //incremental_result.is_err() {

            //result.unwrap().
            //let block_state = executor.take_output_state();

            provider
                .static_file_provider()
                .latest_writer(StaticFileSegment::Headers)
                .unwrap()
                .commit()
                .unwrap();

            provider.commit().unwrap();

            let provider = ProviderFactory::new(
                db.as_ref(),
                Arc::new(case.network.clone().into()),
                static_files_dir_path.clone(),
            )?
            .provider()
            .unwrap();

            //provider.state_provider_by_block_number(block_number)

            let block_state_post = ProviderFactory::new(
                db.as_ref(),
                Arc::new(case.network.clone().into()),
                static_files_dir_path.clone(),
            )?
            .latest()
            .unwrap();

            //block_state_post.state

            let test_address = address!("a94f5374fce5edbc8e2a8697c15331677e6ebf0b");
            println!("account test: {:?}", block_state_pre.basic_account(test_address));
            println!("account test: {:?}", block_state_post.basic_account(test_address));

            println!("{:?}", block_state_pre.proof(test_address, &Vec::new()));
            println!("{:?}", block_state_post.proof(test_address, &Vec::new()));

            println!("num blocks: {}", case.blocks.len());
            let last_block_ = case.blocks.last().unwrap();
            println!("{:?}", last_block_.block_header);
            let first_block = /*case.blocks.last().unwrap().block_header.clone().unwrap().number*/1u64;
            /*let db_state = ProviderFactory::new(
                db.as_ref(),
                Arc::new(case.network.clone().into()),
                static_files_dir_path.clone(),
            )?
            .latest()
            //.unwrap()
            //.state_provider_by_block_number(first_block.try_into().unwrap())
            .unwrap();*/

            //let revm_db = StateProviderDatabase::new(db.as_ref());

            let block_number = first_block.try_into().unwrap();

            let chain_spec = ChainSpec::new_single(
                "test".to_string(),
                1,
                SpecId::SHANGHAI,
                Eip1559Constants::default(),
                false,
            );
            let provider_raiko = RethBlockDataProvider::new(
                provider,
                block_number - 1,
                block_state_pre,
                block_state_post,
                case.pre.clone(),
                State(case.post_state.clone().unwrap_or(case.pre.clone().0)),
            );
            let request = ProofRequest {
                block_number,
                rpc: String::new(),
                l1_rpc: String::new(),
                beacon_rpc: String::new(),
                network: Network::Ethereum,
                graffiti: B256::random(),
                prover: Address::random(),
                l1_network: Network::Ethereum.to_string(),
                proof_type: ProofType::Native,
                prover_args: HashMap::new(),
            };
            let raiko = Raiko::new(chain_spec, request);
            let input = raiko
                .generate_input(provider_raiko)
                .await
                .map_err(|e| Error::Assertion(e.to_string()))?;
            let output = raiko.get_output(&input).map_err(|e| Error::Assertion(e.to_string()));

            //let raiko_result = raiko.prove(chain_spec, provider_raiko, proof_type,
            // request).await; println!("result: {:?}", result);
            //assert!(result.is_ok());

            assert_eq!(execution_result.is_ok(), output.is_ok());

            if output.is_ok() {
                let proof = raiko
                    .prove(input, &output.unwrap())
                    .await
                    .map_err(|e| Error::Assertion(e.to_string()))?;
                let response: NativeResponse = serde_json::from_value(proof).unwrap();
                println!("response: {:?}", response);
                match response.output {
                    GuestOutput::Success { header, hash: _ } => {
                        println!("raiko: {:?}", header.hash_slow());
                        println!("expected: {:?}", case.lastblockhash);
                        assert_eq!(header.hash_slow(), case.lastblockhash);
                    }
                    GuestOutput::Failure => {
                        todo!()
                    }
                };
            }

            // Validate the post-state for the test case.
            match (&case.post_state, &case.post_state_hash) {
                (Some(state), None) => {
                    // Validate accounts in the state against the provider's database.
                    for (&address, account) in state.iter() {
                        /*account.assert_db(address, provider.tx_ref())?;

                        let storage_keys = account.storage.keys().map(|key| B256::new(key.to_le_bytes())).collect::<Vec<_>>();
                        if storage_keys.len() > 0 {
                            let proof = db_state.as_ref().proof(address, &storage_keys).unwrap();
                            let prim_proof = from_primitive_account_proof(proof);
                            //println!("proof: {:?}", prim_proof);
                        }*/
                    }

                    //Ok(())
                }
                (None, Some(expected_state_root)) => {
                    // Insert state hashes into the provider based on the expected state root.
                    let last_block = last_block.unwrap_or_default();
                    /*provider
                    .insert_hashes(
                        0..=last_block.number,
                        last_block.hash(),
                        *expected_state_root,
                    )
                    .map_err(|err| Error::RethError(err.into()))?;*/
                    //Ok(())
                }
                _ => return Err(Error::MissingPostState),
            }

            //println!("genesis_block_header: {:?}", case.genesis_block_header.number);

            /*let block_state = provider.state_provider_by_block_number(block_number).unwrap();
            //block_state.storage(account, storage_key)?;
            block_state.basic_account(addr)

            //provider.block
            provider.block_with_senders(reth_rpc_types::BlockHashOrNumber::Number(block_number), TransactionVariant::NoHash).unwrap();

            let address = Address::default();
            let tx_db: &reth_db::mdbx::tx::Tx<reth_db::mdbx::RO> = provider.tx_ref();
            let account = tx_db.get::<tables::PlainAccountState>(address)?.ok_or_else(|| {
                Error::Assertion(format!("Expected account ({address}) is missing from DB: {self:?}"))
            })?;*/
            //account.

            /*let block_state = ProviderFactory::new(
                db.as_ref(),
                Arc::new(case.network.clone().into()),
                static_files_dir_path.clone(),
            )?
            .provider()
            .unwrap()
            .state_provider_by_block_number(block_number)
            .unwrap();*/

            /*let block_state = ProviderFactory::new(
                db.as_ref(),
                Arc::new(case.network.clone().into()),
                static_files_dir_path.clone(),
            )?
            .latest()
            .unwrap();*/

            // Drop the provider without committing to the database.
            //drop(provider);
            //Ok(())
        }

        Ok(())
    }
}

/// Returns whether the test at the given path should be skipped.
///
/// Some tests are edge cases that cannot happen on mainnet, while others are skipped for
/// convenience (e.g. they take a long time to run) or are temporarily disabled.
///
/// The reason should be documented in a comment above the file name(s).
pub fn should_skip(path: &Path) -> bool {
    let path_str = path.to_str().expect("Path is not valid UTF-8");
    let name = path.file_name().unwrap().to_str().unwrap();
    matches!(
        name,
        // funky test with `bigint 0x00` value in json :) not possible to happen on mainnet and require
        // custom json parser. https://github.com/ethereum/tests/issues/971
        | "ValueOverflow.json"
        | "ValueOverflowParis.json"

        // txbyte is of type 02 and we dont parse tx bytes for this test to fail.
        | "typeTwoBerlin.json"

        // Test checks if nonce overflows. We are handling this correctly but we are not parsing
        // exception in testsuite There are more nonce overflow tests that are in internal
        // call/create, and those tests are passing and are enabled.
        | "CreateTransactionHighNonce.json"

        // Test check if gas price overflows, we handle this correctly but does not match tests specific
        // exception.
        | "HighGasPrice.json"
        | "HighGasPriceParis.json"

        // Skip test where basefee/accesslist/difficulty is present but it shouldn't be supported in
        // London/Berlin/TheMerge. https://github.com/ethereum/tests/blob/5b7e1ab3ffaf026d99d20b17bb30f533a2c80c8b/GeneralStateTests/stExample/eip1559.json#L130
        // It is expected to not execute these tests.
        | "accessListExample.json"
        | "basefeeExample.json"
        | "eip1559.json"
        | "mergeTest.json"

        // These tests are passing, but they take a lot of time to execute so we are going to skip them.
        | "loopExp.json"
        | "Call50000_sha256.json"
        | "static_Call50000_sha256.json"
        | "loopMul.json"
        | "CALLBlake2f_MaxRounds.json"
        | "shiftCombinations.json"
    )
    // Ignore outdated EOF tests that haven't been updated for Cancun yet.
    || path_contains(path_str, &["EIPTests", "stEOF"])
}

/// `str::contains` but for a path. Takes into account the OS path separator (`/` or `\`).
fn path_contains(path_str: &str, rhs: &[&str]) -> bool {
    let rhs = rhs.join(std::path::MAIN_SEPARATOR_STR);
    path_str.contains(&rhs)
}
