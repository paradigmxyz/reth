//! Non-Copy extensions exercise the same account representation used by custom chains.

use alloy_primitives::{keccak256, Address, B256, U256};
use alloy_rlp::{Decodable, Encodable};
use reth_codecs::Compact;
use reth_ethereum_primitives::EthPrimitives;
use reth_execution_cache::{CachedStatus, ExecutionCache};
use reth_primitives_traits::{Account, InMemorySize, NodePrimitives};
use reth_trie_parallel::state_root_task::evm_state_to_hashed_post_state;
use revm::state::{Account as EvmAccount, EvmState};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TestPrimitives;

impl NodePrimitives for TestPrimitives {
    type AccountExtension = TestExtension;
    type Block = <EthPrimitives as NodePrimitives>::Block;
    type BlockHeader = <EthPrimitives as NodePrimitives>::BlockHeader;
    type BlockBody = <EthPrimitives as NodePrimitives>::BlockBody;
    type SignedTx = <EthPrimitives as NodePrimitives>::SignedTx;
    type Receipt = <EthPrimitives as NodePrimitives>::Receipt;
}

#[derive(Clone, Debug)]
struct TestPayload(reth_primitives_traits::SealedBlock<reth_ethereum_primitives::Block>);

impl reth_payload_primitives::BuiltPayload for TestPayload {
    type Primitives = TestPrimitives;

    fn block(&self) -> &reth_primitives_traits::SealedBlock<reth_ethereum_primitives::Block> {
        &self.0
    }

    fn fees(&self) -> U256 {
        U256::ZERO
    }

    fn requests(&self) -> Option<alloy_eips::eip7685::Requests> {
        None
    }
}

impl From<TestPayload> for alloy_rpc_types_engine::ExecutionData {
    fn from(value: TestPayload) -> Self {
        <TestPrimitives as reth_payload_primitives::PayloadTypes>::block_to_payload(value.0, None)
    }
}

impl reth_payload_primitives::PayloadTypes for TestPrimitives {
    type ExecutionData = alloy_rpc_types_engine::ExecutionData;
    type BuiltPayload = TestPayload;
    type PayloadAttributes = alloy_rpc_types_engine::PayloadAttributes;

    fn block_to_payload(
        block: reth_primitives_traits::SealedBlock<reth_ethereum_primitives::Block>,
        bal: Option<alloy_primitives::Bytes>,
    ) -> Self::ExecutionData {
        reth_ethereum_engine_primitives::EthPayloadTypes::block_to_payload(block, bal)
    }
}

type TestNode = reth_node_types::AnyNodeTypes<
    TestPrimitives,
    reth_chainspec::ChainSpec,
    reth_provider::EthStorage,
    TestPrimitives,
>;

#[derive(Clone, Debug)]
struct TestEvm(reth_evm_ethereum::EthEvmConfig);

impl reth_evm::ConfigureEvm for TestEvm {
    type Primitives = TestPrimitives;
    type Error = <reth_evm_ethereum::EthEvmConfig as reth_evm::ConfigureEvm>::Error;
    type NextBlockEnvCtx = reth_evm::NextBlockEnvAttributes;
    type BlockExecutorFactory =
        <reth_evm_ethereum::EthEvmConfig as reth_evm::ConfigureEvm>::BlockExecutorFactory;
    type BlockAssembler =
        <reth_evm_ethereum::EthEvmConfig as reth_evm::ConfigureEvm>::BlockAssembler;

    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory {
        self.0.block_executor_factory()
    }

    fn block_assembler(&self) -> &Self::BlockAssembler {
        self.0.block_assembler()
    }

    fn evm_env(
        &self,
        header: &alloy_consensus::Header,
    ) -> Result<reth_evm::EvmEnvFor<Self>, Self::Error> {
        self.0.evm_env(header)
    }

    fn next_evm_env(
        &self,
        parent: &alloy_consensus::Header,
        attributes: &Self::NextBlockEnvCtx,
    ) -> Result<reth_evm::EvmEnvFor<Self>, Self::Error> {
        self.0.next_evm_env(parent, attributes)
    }

    fn context_for_block<'a>(
        &self,
        block: &'a reth_primitives_traits::SealedBlock<reth_ethereum_primitives::Block>,
    ) -> Result<reth_evm::ExecutionCtxFor<'a, Self>, Self::Error> {
        self.0.context_for_block(block)
    }

    fn context_for_next_block(
        &self,
        parent: &reth_primitives_traits::SealedHeader,
        attributes: Self::NextBlockEnvCtx,
    ) -> Result<reth_evm::ExecutionCtxFor<'_, Self>, Self::Error> {
        self.0.context_for_next_block(parent, attributes)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, Compact)]
pub(super) struct TestExtension {
    pub(super) value: B256,
}

impl InMemorySize for TestExtension {
    fn size(&self) -> usize {
        size_of::<Self>()
    }
}

impl alloy_trie::TrieAccountExtension for TestExtension {
    fn payload_length(&self) -> usize {
        self.value.length()
    }

    fn encode_payload(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.value.encode(out);
    }

    fn decode_payload(payload: &mut &[u8]) -> alloy_rlp::Result<Self> {
        B256::decode(payload).map(|value| Self { value })
    }
}

#[test]
fn extension_only_execution_update_is_not_lost() {
    let address = Address::repeat_byte(1);
    let before = Account {
        balance: U256::from(100),
        extension: TestExtension { value: B256::repeat_byte(2) },
        ..Default::default()
    };
    let after = Account { extension: TestExtension { value: B256::repeat_byte(3) }, ..before };
    let mut evm_account = EvmAccount::from(revm::state::AccountInfo::from(before));
    evm_account.set_current_info_as_original();
    evm_account.info = after.clone().into();
    evm_account.mark_touch();
    let hashed = evm_state_to_hashed_post_state::<TestExtension>(EvmState::from_iter([(
        address,
        evm_account,
    )]));
    assert_eq!(hashed.accounts.get(&keccak256(address)), Some(&Some(after)));
}

#[test]
fn account_cache_preserves_non_copy_extension() {
    let address = Address::repeat_byte(1);
    let account =
        Account { extension: TestExtension { value: B256::repeat_byte(4) }, ..Default::default() };
    let cache = ExecutionCache::new(1024 * 1024);
    cache.insert_account(address, Some(account.clone()));
    let hit = cache
        .get_or_try_insert_account_with::<()>(address, || panic!("account must be cached"))
        .unwrap();
    assert!(matches!(hit, CachedStatus::Cached(Some(cached)) if cached == account));
}

#[test]
fn account_compact_preserves_extension() {
    let account = Account {
        nonce: 7,
        extension: TestExtension { value: B256::repeat_byte(5) },
        ..Default::default()
    };
    let mut bytes = Vec::new();
    let len = account.to_compact(&mut bytes);
    let (decoded, rest) = Account::<TestExtension>::from_compact(&bytes, len);
    assert_eq!(decoded, account);
    assert!(rest.is_empty());

    let changeset =
        reth_db::models::AccountBeforeTx { address: Address::repeat_byte(1), info: Some(account) };
    bytes.clear();
    let len = changeset.to_compact(&mut bytes);
    let (decoded, rest) =
        reth_db::models::AccountBeforeTx::<TestExtension>::from_compact(&bytes, len);
    assert_eq!(decoded, changeset);
    assert!(rest.is_empty());
}

#[test]
fn txpool_prewarm_snapshot_preserves_extension() {
    let address = Address::repeat_byte(1);
    let account =
        Account { extension: TestExtension { value: B256::repeat_byte(9) }, ..Default::default() };
    let mut reads = reth_revm::cached::CachedReads::default();
    reads.insert_account(address, account.clone().into(), Default::default());
    let snapshot = reth_execution_cache::TxPoolPrewarmCacheSnapshot::new(
        B256::ZERO,
        std::sync::Arc::new(reads),
    );
    assert_eq!(snapshot.account::<TestExtension>(&address), Some(Some(account)));
}

#[test]
fn unsupported_protocols_check_type_not_feature_or_value() {
    use reth_primitives_traits::EmptyAccountExtension;
    use reth_provider::{ensure_no_account_extensions, ProviderError};

    for protocol in ["BAL", "snap"] {
        assert!(ensure_no_account_extensions::<EmptyAccountExtension>(protocol).is_ok());
        let error = ensure_no_account_extensions::<TestExtension>(protocol).unwrap_err();
        assert!(
            matches!(error, ProviderError::AccountExtensionsUnsupported(name) if name == protocol)
        );
        assert_eq!(error.to_string(), format!("{protocol} does not support account extensions"));
    }
}

#[test]
fn database_and_state_provider_preserve_custom_accounts() {
    use reth_db::{models::AccountBeforeTx, tables, transaction::DbTxMut};
    use reth_provider::{AccountReader, ChangeSetReader, StateRangeProviderFactory};

    let factory = reth_provider::test_utils::create_test_provider_factory_with_node_types::<TestNode>(
        reth_chainspec::MAINNET.clone(),
    );
    let address = Address::repeat_byte(1);
    let account = Account {
        balance: U256::from(100),
        extension: TestExtension { value: B256::repeat_byte(6) },
        ..Default::default()
    };
    let provider = factory.provider_rw().unwrap();
    provider
        .tx_ref()
        .put::<tables::PlainAccountState<TestExtension>>(address, account.clone())
        .unwrap();
    let before = AccountBeforeTx { address, info: Some(account.clone()) };
    provider.tx_ref().put::<tables::AccountChangeSets<TestExtension>>(1, before.clone()).unwrap();
    assert_eq!(provider.basic_account(&address).unwrap(), Some(account.clone()));
    assert_eq!(provider.account_block_changeset(1).unwrap(), vec![before]);
    provider.commit().unwrap();

    let state = factory.latest().unwrap();
    assert_eq!(state.basic_account(&address).unwrap(), Some(account));
    drop(state);
    let blockchain =
        reth_provider::providers::BlockchainProvider::with_latest(factory, Default::default())
            .unwrap();
    assert!(matches!(
        blockchain.state_range_provider(B256::ZERO),
        Err(reth_provider::ProviderError::AccountExtensionsUnsupported("snap"))
    ));
}

#[test]
fn table_dispatch_selects_custom_account_type() {
    struct ValueType;
    impl reth_db::TableViewer<&'static str> for ValueType {
        type Error = std::convert::Infallible;
        type AccountExtension = TestExtension;
        fn view<T: reth_db::table::Table>(&self) -> Result<&'static str, Self::Error> {
            Ok(std::any::type_name::<T::Value>())
        }
    }
    for table in [
        reth_db::tables::Tables::PlainAccountState,
        reth_db::tables::Tables::HashedAccounts,
        reth_db::tables::Tables::AccountChangeSets,
    ] {
        let value = table.view(&ValueType).unwrap();
        assert!(value.contains("TestExtension"), "wrong table value type: {value}");
    }
}

#[test]
fn payload_builder_and_validation_hash_the_custom_account() {
    use reth_db::{tables, transaction::DbTxMut};
    use reth_evm::{execute::BlockBuilder, ConfigureEvm};
    use reth_provider::{AccountReader, StateRootProvider, StateWriter};
    use reth_trie::HashedPostState;

    let chain_spec =
        std::sync::Arc::new(reth_chainspec::ChainSpecBuilder::mainnet().paris_activated().build());
    let factory = reth_provider::test_utils::create_test_provider_factory_with_node_types::<TestNode>(
        chain_spec.clone(),
    );
    let address = Address::repeat_byte(1);
    let original = Account {
        balance: U256::from(100),
        extension: TestExtension { value: B256::repeat_byte(7) },
        ..Default::default()
    };
    let provider = factory.provider_rw().unwrap();
    provider
        .tx_ref()
        .put::<tables::PlainAccountState<TestExtension>>(address, original.clone())
        .unwrap();
    let original_hashed = HashedPostState {
        accounts: std::iter::once((keccak256(address), Some(original.clone()))).collect(),
        ..Default::default()
    };
    provider.write_hashed_state(&original_hashed.into_sorted()).unwrap();
    provider.commit().unwrap();

    let state_provider = factory.latest().unwrap();
    let before_root = state_provider.state_root(HashedPostState::default()).unwrap();
    let mut changed = original.clone();
    changed.extension.value = B256::repeat_byte(8);
    let expected = HashedPostState {
        accounts: std::iter::once((keccak256(address), Some(changed.clone()))).collect(),
        ..Default::default()
    };
    let validation_root = state_provider.state_root(expected).unwrap();
    assert_ne!(before_root, validation_root);

    let mut state = revm::database::State::builder()
        .with_database(reth_revm::database::StateProviderDatabase::new(&state_provider))
        .with_bundle_update()
        .build();
    // Models a chain-specific execution hook changing only the extension.
    state.bundle_state = revm::database::BundleState::new(
        [(
            address,
            Some(original.clone().into()),
            Some(changed.clone().into()),
            Default::default(),
        )],
        [[(address, Some(Some(original.into())), Vec::<(U256, U256)>::new())]],
        [],
    );
    let evm = TestEvm(reth_evm_ethereum::EthEvmConfig::new(chain_spec));
    let parent = reth_primitives_traits::SealedHeader::default();
    let attributes = reth_evm::NextBlockEnvAttributes {
        timestamp: 1,
        suggested_fee_recipient: Address::ZERO,
        prev_randao: B256::ZERO,
        gas_limit: 30_000_000,
        parent_beacon_block_root: None,
        withdrawals: None,
        extra_data: Default::default(),
        slot_number: None,
    };
    let builder = evm.builder_for_next_block(&mut state, &parent, attributes.clone()).unwrap();
    let outcome = builder.finish(&state_provider, None).unwrap();
    assert_eq!(outcome.hashed_state.accounts.get(&keccak256(address)), Some(&Some(changed)));
    assert_eq!(outcome.block.header().state_root, validation_root);
    assert_eq!(
        state_provider.basic_account(&address).unwrap().unwrap().extension.value,
        B256::repeat_byte(7)
    );

    state.bal_state = revm::database_interface::bal::BalState::new().with_bal_builder();
    let builder = evm.builder_for_next_block(&mut state, &parent, attributes).unwrap();
    let error = builder.finish(&state_provider, None).unwrap_err();
    assert!(error.to_string().contains("BAL does not support account extensions"));
}
