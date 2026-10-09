//! T-014: account and chunk records share the publication transaction.

use alloy_primitives::{keccak256, Address, Bytes};
use reth_db::test_utils::DatabaseTestHooks;
use reth_db_api::{tables, transaction::DbTx};
use reth_primitives_traits::{Account, Bytecode};
use reth_provider::{
    test_utils::{create_test_provider_factory, create_test_provider_factory_with_db_hooks},
    StateProviderFactory,
};
use reth_storage_api::{
    AccountReader, BytecodeReader, CodeChunkReader, DBProvider, MetadataWriter, StorageSettings,
    StorageSettingsCache, ValidatedCode,
};

#[test]
fn t014_pending_abort_and_commit_visibility() {
    for settings in [StorageSettings::v1(), StorageSettings::v2()] {
        for replace in [false, true] {
            for commit in [false, true] {
                let factory = create_test_provider_factory();
                let setup = factory.provider_rw().unwrap();
                setup.write_storage_settings(settings).unwrap();
                setup.commit().unwrap();
                factory.set_storage_settings_cache(settings);
                let address = Address::repeat_byte(0x43);
                let old_bytes = Bytes::from_static(&[0x01]);
                let old_hash = keccak256(&old_bytes);
                let old_account =
                    Account { nonce: 3, bytecode_hash: Some(old_hash), ..Default::default() };
                if replace {
                    let writer = factory.provider_rw().unwrap();
                    writer
                        .write_chunked_code(
                            address,
                            old_account.clone(),
                            &ValidatedCode::new(old_bytes.clone()).unwrap(),
                        )
                        .unwrap();
                    writer.commit().unwrap();
                }
                let snapshot = factory.latest().unwrap();
                let original = Bytes::from(vec![0; 24542]);
                let hash = keccak256(&original);
                let account = Account { nonce: 4, bytecode_hash: Some(hash), ..Default::default() };
                let writer = factory.provider_rw().unwrap();
                writer
                    .write_chunked_code(
                        address,
                        account.clone(),
                        &ValidatedCode::new(original.clone()).unwrap(),
                    )
                    .unwrap();
                // A newly opened reader, not merely the existing MVCC snapshot.
                {
                    let pending = factory.latest().unwrap();
                    assert_eq!(
                        pending.basic_account(&address).unwrap(),
                        replace.then_some(old_account.clone())
                    );
                    assert_eq!(pending.get_code_chunk_by_hash(&hash, 0).unwrap(), None);
                    let raw = factory.provider().unwrap();
                    assert!(raw
                        .tx_ref()
                        .get::<tables::BytecodeChunkDescriptors>(hash)
                        .unwrap()
                        .is_none());
                    assert!(raw
                        .tx_ref()
                        .get::<tables::BytecodeChunks>(keccak256(&original[..24541]))
                        .unwrap()
                        .is_none());
                }
                if commit {
                    writer.commit().unwrap();
                } else {
                    drop(writer);
                }
                let current = factory.latest().unwrap();
                assert_eq!(
                    snapshot.basic_account(&address).unwrap(),
                    replace.then_some(old_account.clone())
                );
                if commit {
                    assert_eq!(current.basic_account(&address).unwrap(), Some(account));
                    assert_eq!(
                        current.get_code_chunk_by_hash(&hash, 1).unwrap(),
                        Some(Bytes::from_static(&[0]))
                    );
                    assert_eq!(
                        current.bytecode_by_hash(&hash).unwrap().unwrap().original_bytes(),
                        original
                    );
                } else {
                    assert_eq!(
                        current.basic_account(&address).unwrap(),
                        replace.then_some(old_account.clone())
                    );
                    assert_eq!(current.get_code_chunk_by_hash(&hash, 0).unwrap(), None);
                }
                if replace {
                    assert_eq!(
                        snapshot.bytecode_by_hash(&old_hash).unwrap(),
                        Some(Bytecode::new_raw(old_bytes))
                    );
                }
            }
        }
    }
}

/// T-015 uses a low-level write fault and requires production commit refusal.
#[test]
fn t015_failed_write_poisoning_prevents_partial_commit() {
    for settings in [StorageSettings::v1(), StorageSettings::v2()] {
        let original = Bytes::from(vec![0; 24542]);
        let hash = keccak256(&original);
        let code = ValidatedCode::new(original).unwrap();
        let address = Address::repeat_byte(0x15);
        let account = Account { bytecode_hash: Some(hash), ..Default::default() };
        let control = DatabaseTestHooks::default();
        let factory = create_test_provider_factory_with_db_hooks(control.clone());
        let setup = factory.provider_rw().unwrap();
        setup.write_storage_settings(settings).unwrap();
        setup.commit().unwrap();
        factory.set_storage_settings_cache(settings);
        let writer = factory.provider_rw().unwrap();
        control.reset_write_count();
        writer.write_chunked_code(address, account.clone(), &code).unwrap();
        let write_count = control.write_count();
        assert!(write_count >= 4, "two payloads, descriptor, and account must persist");
        writer.commit().unwrap();
        for failure in 1..=write_count {
            let hooks = DatabaseTestHooks::default();
            let factory = create_test_provider_factory_with_db_hooks(hooks.clone());
            let setup = factory.provider_rw().unwrap();
            setup.write_storage_settings(settings).unwrap();
            setup.commit().unwrap();
            factory.set_storage_settings_cache(settings);
            let writer = factory.provider_rw().unwrap();
            hooks.fail_nth_write(failure);
            assert!(writer.write_chunked_code(address, account.clone(), &code).is_err());
            assert_eq!(hooks.injected_failures(), 1);
            hooks.disable_write_failure();
            // Exercise production refusal; test-owned drop is not the abort mechanism.
            assert!(writer.commit().is_err());
            let state = factory.latest().unwrap();
            assert_eq!(state.basic_account(&address).unwrap(), None);
            assert_eq!(state.get_code_chunk_by_hash(&hash, 0).unwrap(), None);
            let reader = factory.provider().unwrap();
            assert_eq!(reader.tx_ref().entries::<tables::BytecodeChunks>().unwrap(), 0);
            assert_eq!(reader.tx_ref().entries::<tables::BytecodeChunkDescriptors>().unwrap(), 0);
        }
    }
}

/// T-015 replacement failures preserve an already published multi-chunk identity.
#[test]
fn t015_replacement_failure_preserves_previous_complete_state() {
    for settings in [StorageSettings::v1(), StorageSettings::v2()] {
        let address = Address::repeat_byte(0x51);
        let old_bytes = Bytes::from(vec![0; 24542]);
        let old_hash = keccak256(&old_bytes);
        let old_code = ValidatedCode::new(old_bytes.clone()).unwrap();
        let old_account = Account {
            nonce: 9,
            bytecode_hash: Some(old_hash),
            extension: Bytes::from_static(&[0xfe, 0x51]).into(),
            ..Default::default()
        };
        let mut replacement = vec![0; 49083];
        replacement[0] = 0x01;
        replacement[24541] = 0x02;
        replacement[49082] = 0x03;
        let replacement = Bytes::from(replacement);
        let hash = keccak256(&replacement);
        let code = ValidatedCode::new(replacement.clone()).unwrap();
        let account = Account {
            nonce: 10,
            bytecode_hash: Some(hash),
            extension: Bytes::from_static(&[0xfe, 0x52]).into(),
            ..Default::default()
        };
        // Measure the actual write sequence, including an injection-disabled success control.
        let hooks = DatabaseTestHooks::default();
        let factory = create_test_provider_factory_with_db_hooks(hooks.clone());
        let setup = factory.provider_rw().unwrap();
        setup.write_storage_settings(settings).unwrap();
        setup.commit().unwrap();
        factory.set_storage_settings_cache(settings);
        let writer = factory.provider_rw().unwrap();
        writer.write_chunked_code(address, old_account.clone(), &old_code).unwrap();
        writer.commit().unwrap();
        let writer = factory.provider_rw().unwrap();
        hooks.reset_write_count();
        writer.write_chunked_code(address, account.clone(), &code).unwrap();
        let writes = hooks.write_count();
        assert!(writes >= 5);
        writer.commit().unwrap();
        assert_eq!(
            factory.latest().unwrap().basic_account(&address).unwrap(),
            Some(account.clone())
        );
        assert_eq!(
            factory.latest().unwrap().bytecode_by_hash(&hash).unwrap().unwrap().original_bytes(),
            replacement
        );

        for failure in 1..=writes {
            let hooks = DatabaseTestHooks::default();
            let factory = create_test_provider_factory_with_db_hooks(hooks.clone());
            let setup = factory.provider_rw().unwrap();
            setup.write_storage_settings(settings).unwrap();
            setup.commit().unwrap();
            factory.set_storage_settings_cache(settings);
            let writer = factory.provider_rw().unwrap();
            writer.write_chunked_code(address, old_account.clone(), &old_code).unwrap();
            writer.commit().unwrap();
            let snapshot = factory.latest().unwrap();
            let writer = factory.provider_rw().unwrap();
            hooks.fail_nth_write(failure);
            assert!(writer.write_chunked_code(address, account.clone(), &code).is_err());
            assert_eq!(hooks.injected_failures(), 1);
            // Open a fresh reader while failed writes are still pending.
            let pending = factory.latest().unwrap();
            assert_eq!(pending.basic_account(&address).unwrap(), Some(old_account.clone()));
            assert_eq!(pending.get_code_chunk_by_hash(&hash, 0).unwrap(), None);
            drop(pending);
            hooks.disable_write_failure();
            assert!(writer.commit().is_err(), "failure {failure} must poison publication");
            let current = factory.latest().unwrap();
            for state in [&snapshot, &current] {
                assert_eq!(state.basic_account(&address).unwrap(), Some(old_account.clone()));
                assert_eq!(
                    state.bytecode_by_hash(&old_hash).unwrap().unwrap().original_bytes(),
                    old_bytes
                );
                for (index, payload) in old_bytes.chunks(24541).enumerate() {
                    assert_eq!(
                        state
                            .get_code_chunk_by_hash(&old_hash, index as u32)
                            .unwrap()
                            .unwrap()
                            .as_ref(),
                        payload
                    );
                }
                assert_eq!(state.get_code_chunk_by_hash(&hash, 0).unwrap(), None);
            }
            let raw = factory.provider().unwrap();
            assert_eq!(raw.tx_ref().entries::<tables::BytecodeChunkDescriptors>().unwrap(), 1);
            assert_eq!(raw.tx_ref().entries::<tables::BytecodeChunks>().unwrap(), 2);
            for payload in replacement.chunks(24541) {
                assert_eq!(
                    raw.tx_ref().get::<tables::BytecodeChunks>(keccak256(payload)).unwrap(),
                    None
                );
            }
        }
    }
}

#[test]
fn authenticated_staging_failure_poison_prevents_partial_commit() {
    for failure in 1..=3 {
        let hooks = DatabaseTestHooks::default();
        let factory = create_test_provider_factory_with_db_hooks(hooks.clone());
        let mut bytes = vec![0; 24542];
        bytes[24540] = 0x60;
        bytes[24541] = 0xa7;
        let code = ValidatedCode::new(bytes.into()).unwrap();
        let writer = factory.provider_rw().unwrap();
        hooks.fail_nth_write(failure);
        assert!(
            reth_storage_api::StateWriter::write_validated_chunked_code(&*writer, &code).is_err()
        );
        hooks.disable_write_failure();
        assert!(writer.commit().is_err());
        let reader = factory.provider().unwrap();
        assert_eq!(reader.tx_ref().entries::<tables::BytecodeChunks>().unwrap(), 0);
        assert_eq!(reader.tx_ref().entries::<tables::BytecodeChunkDescriptors>().unwrap(), 0);
    }
}
