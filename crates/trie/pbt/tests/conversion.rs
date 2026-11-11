//! Source-MPT/preimage merging and authenticated conversion.

#![cfg(feature = "conversion")]

use alloy_primitives::{keccak256, map::B256Map, Address, Bytes, B256, U256};
use reth_primitives_traits::{Account, Bytecode};
use reth_trie::hashed_cursor::mock::MockHashedCursorFactory;
use reth_trie_pbt::{
    conversion::{convert_mpt, BytecodeReader, ConversionError, ProviderError},
    migration::{mpt_root, ArtifactError, Artifacts, DecodeLimits},
    AccountData, Blake3Hasher, StateTree,
};
use std::collections::BTreeMap;

#[derive(Default)]
struct CodeStore(B256Map<Bytecode>);

impl BytecodeReader for CodeStore {
    fn bytecode_by_hash(&self, hash: &B256) -> Result<Option<Bytecode>, ProviderError> {
        Ok(self.0.get(hash).cloned())
    }
}

#[test]
fn source_conversion_matches_independent_state_and_artifacts() {
    let mut state = StateTree::<Blake3Hasher>::new();
    for byte in [1, 2, 3] {
        let address = Address::repeat_byte(byte);
        let mut code = if byte == 3 { vec![0xef, 1, 0] } else { vec![0x7f; 65] };
        if byte == 3 {
            code.extend_from_slice(Address::ZERO.as_slice());
        }
        state
            .set_account(
                address,
                AccountData { nonce: 1, balance: U256::from(byte), code: code.into() },
            )
            .unwrap();
        for slot in [0, 63, 64, 255, 256, u64::MAX] {
            state.set_storage(address, U256::from(slot), B256::from(U256::from(byte))).unwrap();
        }
    }
    let (source, codes, artifacts) = source(&state);
    let converted = convert_mpt::<Blake3Hasher>(
        &source,
        &codes,
        &artifacts.preimages,
        mpt_root(&state),
        DecodeLimits::default(),
        || false,
    )
    .unwrap();
    assert_eq!(converted.root(), state.root());
    assert_eq!(Artifacts::from_state(&converted).unwrap(), artifacts);
}

#[test]
fn missing_and_surplus_account_preimages_name_the_key() {
    let mut state = StateTree::<Blake3Hasher>::new();
    state.set_account(Address::ZERO, AccountData { nonce: 1, ..Default::default() }).unwrap();
    let (source, codes, artifacts) = source(&state);
    assert!(matches!(
        convert_mpt::<Blake3Hasher>(&source, &codes, &[], mpt_root(&state), DecodeLimits::default(), || false),
        Err(ConversionError::MissingAccount(hash)) if hash == keccak256(Address::ZERO)
    ));
    assert!(matches!(
        convert_mpt::<Blake3Hasher>(&MockHashedCursorFactory::default(), &codes, &artifacts.preimages, mpt_root(&state), DecodeLimits::default(), || false),
        Err(ConversionError::SurplusAccount(address)) if address == Address::ZERO
    ));
}

#[test]
fn missing_and_surplus_storage_preimages_are_rejected() {
    let mut state = StateTree::<Blake3Hasher>::new();
    state.set_account(Address::ZERO, AccountData { nonce: 1, ..Default::default() }).unwrap();
    let (empty_source, _, empty) = source(&state);
    state.set_storage(Address::ZERO, U256::from(64), B256::repeat_byte(1)).unwrap();
    let (source, codes, artifacts) = source(&state);
    assert!(matches!(
        convert_mpt::<Blake3Hasher>(&source, &codes, &empty.preimages, mpt_root(&state), DecodeLimits::default(), || false),
        Err(ConversionError::MissingSlot { address: Address::ZERO, slot_hash }) if slot_hash == keccak256(B256::from(U256::from(64)))
    ));
    assert!(matches!(
        convert_mpt::<Blake3Hasher>(&empty_source, &codes, &artifacts.preimages, mpt_root(&state), DecodeLimits::default(), || false),
        Err(ConversionError::SurplusSlot { address: Address::ZERO, slot }) if slot == B256::from(U256::from(64))
    ));
}

#[test]
fn corrupt_code_anchor_and_resource_budgets_fail_before_publication() {
    let mut state = StateTree::<Blake3Hasher>::new();
    state
        .set_account(
            Address::ZERO,
            AccountData { nonce: 1, code: Bytes::from_static(&[0x60, 1]), ..Default::default() },
        )
        .unwrap();
    let (source, mut codes, artifacts) = source(&state);
    let hash = keccak256([0x60, 1]);
    for code in [None, Some(Bytecode::new_raw(Bytes::from_static(&[0x00])))] {
        codes.0.remove(&hash);
        if let Some(code) = code {
            codes.0.insert(hash, code);
        }
        assert!(matches!(
            convert_mpt::<Blake3Hasher>(&source, &codes, &artifacts.preimages, mpt_root(&state), DecodeLimits::default(), || false),
            Err(ConversionError::MissingCode(actual)) if actual == hash
        ));
    }
    codes.0.insert(hash, Bytecode::new_raw(Bytes::from_static(&[0x60, 1])));
    assert!(matches!(
        convert_mpt::<Blake3Hasher>(
            &source,
            &codes,
            &artifacts.preimages,
            B256::ZERO,
            DecodeLimits::default(),
            || false
        ),
        Err(ConversionError::Artifact(ArtifactError::AnchorRootMismatch))
    ));
    for limits in [
        DecodeLimits { max_accounts: 0, ..Default::default() },
        DecodeLimits { max_leaves: 0, ..Default::default() },
        DecodeLimits { max_code_size: 0, ..Default::default() },
        DecodeLimits { max_total_code_size: 0, ..Default::default() },
    ] {
        assert!(matches!(
            convert_mpt::<Blake3Hasher>(
                &source,
                &codes,
                &artifacts.preimages,
                mpt_root(&state),
                limits,
                || false
            ),
            Err(ConversionError::Artifact(ArtifactError::ResourceLimit))
        ));
    }
    assert!(matches!(
        convert_mpt::<Blake3Hasher>(
            &source,
            &codes,
            &artifacts.preimages,
            mpt_root(&state),
            DecodeLimits::default(),
            || true
        ),
        Err(ConversionError::Cancelled)
    ));
}

fn source(state: &StateTree) -> (MockHashedCursorFactory, CodeStore, Artifacts) {
    let mut accounts = BTreeMap::new();
    let mut storages = B256Map::default();
    let mut codes = CodeStore::default();
    for (address, account) in state.accounts() {
        let hash = keccak256(address);
        let code_hash = (!account.code.is_empty()).then(|| keccak256(&account.code));
        accounts.insert(hash, Account::new(account.nonce, account.balance, code_hash));
        storages.insert(
            hash,
            state
                .storage_entries(address)
                .map(|(slot, value)| (keccak256(B256::from(slot)), value.into()))
                .collect(),
        );
        if let Some(code_hash) = code_hash {
            codes.0.insert(code_hash, Bytecode::new_raw(account.code.clone()));
        }
    }
    (MockHashedCursorFactory::new(accounts, storages), codes, Artifacts::from_state(state).unwrap())
}

#[test]
fn malformed_preimages_and_mid_conversion_cancellation_are_rejected() {
    let mut state = StateTree::<Blake3Hasher>::new();
    state.set_account(Address::ZERO, AccountData { nonce: 1, ..Default::default() }).unwrap();
    let (accounts, codes, artifacts) = source(&state);
    for length in 1..artifacts.preimages.len() {
        assert!(matches!(
            convert_mpt::<Blake3Hasher>(
                &accounts,
                &codes,
                &artifacts.preimages[..length],
                mpt_root(&state),
                DecodeLimits::default(),
                || false
            ),
            Err(ConversionError::Artifact(ArtifactError::Truncated))
        ));
    }
    let duplicate = [artifacts.preimages.as_slice(), artifacts.preimages.as_slice()].concat();
    assert!(matches!(
        convert_mpt::<Blake3Hasher>(
            &accounts,
            &codes,
            &duplicate,
            mpt_root(&state),
            DecodeLimits::default(),
            || false
        ),
        Err(ConversionError::Artifact(ArtifactError::Invalid(
            "preimage keys not strictly ordered"
        )))
    ));
    state.set_storage(Address::ZERO, U256::from(64), B256::repeat_byte(1)).unwrap();
    let (accounts, codes, artifacts) = source(&state);
    let mut duplicate = artifacts.preimages.clone();
    duplicate[20..24].copy_from_slice(&2u32.to_be_bytes());
    duplicate.extend_from_slice(&artifacts.preimages[24..]);
    assert!(matches!(
        convert_mpt::<Blake3Hasher>(
            &accounts,
            &codes,
            &duplicate,
            mpt_root(&state),
            DecodeLimits::default(),
            || false
        ),
        Err(ConversionError::Artifact(ArtifactError::Invalid(
            "preimage keys not strictly ordered"
        )))
    ));
    let mut polls = 0;
    assert!(matches!(
        convert_mpt::<Blake3Hasher>(
            &accounts,
            &codes,
            &artifacts.preimages,
            mpt_root(&state),
            DecodeLimits::default(),
            || {
                polls += 1;
                polls == 3
            }
        ),
        Err(ConversionError::Cancelled)
    ));
    assert_eq!(polls, 3);
}

#[test]
fn delegation_does_not_consume_contract_chunk_budgets() {
    let mut code = vec![0xef, 1, 0];
    code.extend_from_slice(Address::ZERO.as_slice());
    let mut state = StateTree::<Blake3Hasher>::new();
    state
        .set_account(Address::ZERO, AccountData { code: code.into(), ..Default::default() })
        .unwrap();
    let (source, codes, artifacts) = source(&state);
    let converted = convert_mpt::<Blake3Hasher>(
        &source,
        &codes,
        &artifacts.preimages,
        mpt_root(&state),
        DecodeLimits { max_code_size: 0, max_total_code_size: 0, ..Default::default() },
        || false,
    )
    .unwrap();
    assert_eq!(converted.root(), state.root());
}
