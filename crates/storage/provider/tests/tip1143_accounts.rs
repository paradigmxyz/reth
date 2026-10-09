//! T-017: extension bytes are opaque to the generic chunk writer.

use alloy_primitives::{keccak256, Address, Bytes, U256};
use reth_codecs::Compact;
use reth_primitives_traits::Account;
use reth_provider::{test_utils::create_test_provider_factory, StateProviderFactory};
use reth_storage_api::{AccountReader, DBProvider, EvmStateProviderAdapter, ValidatedCode};
use reth_trie::{root::state_root_unhashed, EMPTY_ROOT_HASH};

#[test]
fn t017_opaque_extensions_survive_persistence_and_compact() {
    for bytes in [vec![], vec![1, 0, 0x60, 1, 0xff], vec![0xfe, 0, 1, 2, 3]] {
        let factory = create_test_provider_factory();
        let original = Bytes::from(vec![0; 24542]);
        let account = Account {
            nonce: 19,
            balance: U256::from(1143),
            bytecode_hash: Some(keccak256(&original)),
            extension: Bytes::from(bytes).into(),
        };
        let mut expected_compact = Vec::new();
        account.to_compact(&mut expected_compact);
        let writer = factory.provider_rw().unwrap();
        writer
            .write_chunked_code(
                Address::ZERO,
                account.clone(),
                &ValidatedCode::new(original).unwrap(),
            )
            .unwrap();
        writer.commit().unwrap();
        let actual = factory.latest().unwrap().basic_account(&Address::ZERO).unwrap().unwrap();
        assert_eq!(actual, account);
        let mut compact = Vec::new();
        actual.to_compact(&mut compact);
        assert_eq!(compact, expected_compact);
        let (decoded, remainder) = Account::from_compact(&compact, compact.len());
        assert!(remainder.is_empty());
        assert_eq!(decoded, account);
    }
}

/// T-017: exercise the actual trie and both execution database conversions.
#[test]
fn t017_trie_and_execution_accounts_preserve_opaque_extensions() {
    let factory = create_test_provider_factory();
    let original = Bytes::from(vec![0; 24542]);
    let hash = keccak256(&original);
    let code = ValidatedCode::new(original).unwrap();
    let address = Address::repeat_byte(0x17);
    let mut roots = Vec::new();
    for extension in [vec![], vec![1, 0, 0x60, 1, 0xff], vec![0xfe, 0, 1, 2, 3]] {
        let account = Account {
            nonce: 19,
            balance: U256::from(1143),
            bytecode_hash: Some(hash),
            extension: Bytes::from(extension.clone()).into(),
        };
        let writer = factory.provider_rw().unwrap();
        writer.write_chunked_code(address, account.clone(), &code).unwrap();
        writer.commit().unwrap();
        let persisted = factory.latest().unwrap().basic_account(&address).unwrap().unwrap();
        let trie = persisted.into_trie_account(EMPTY_ROOT_HASH);
        assert_eq!(trie.extension.as_ref(), extension.as_slice());
        assert_eq!(trie.nonce, 19);
        assert_eq!(trie.balance, U256::from(1143));
        assert_eq!(trie.code_hash, hash);
        assert_eq!(trie.storage_root, EMPTY_ROOT_HASH);
        roots.push(state_root_unhashed([(address, trie)]));

        let mut evm = reth_evm::database::StateProviderDatabase::new(EvmStateProviderAdapter(
            factory.latest().unwrap(),
        ));
        let info = reth_evm::Database::get_account(&mut evm, &address).unwrap().unwrap();
        assert_eq!(info.extension.as_ref(), extension.as_slice());
        assert_eq!(info.nonce, 19);
        assert_eq!(info.balance, U256::from(1143));
        assert_eq!(info.code_hash, hash);
        assert!(info.code.is_none());

        let revm = reth_revm::database::StateProviderDatabase::new(EvmStateProviderAdapter(
            factory.latest().unwrap(),
        ));
        let info = revm::DatabaseRef::basic_ref(&revm, address).unwrap().unwrap();
        assert_eq!(info.extension.as_ref(), extension.as_slice());
        assert_eq!(info.nonce, 19);
        assert_eq!(info.balance, U256::from(1143));
        assert_eq!(info.code_hash, hash);
        assert!(info.code.is_none());
        assert_eq!(Account::from(info), account);
    }
    assert_eq!(roots.len(), 3);
    assert_ne!(roots[0], roots[1]);
    assert_ne!(roots[0], roots[2]);
    assert_ne!(roots[1], roots[2]);
}
