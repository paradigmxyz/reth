use super::*;
use alloy_primitives::StorageKey;
use reth_ethereum_primitives::EthPrimitives;
use reth_primitives_traits::{Account, Bytecode as ProviderBytecode};
use reth_provider::{test_utils::MockEthProvider, EvmStateProvider};
use reth_storage_overlay::OverlayManager;
use revm::state::bal::Bal;
use std::sync::Mutex;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Call {
    Account(Address),
    Storage(Address, U256),
}

struct Provider {
    calls: Arc<Mutex<Vec<Call>>>,
    base: U256,
    _lifetime: Arc<()>,
}

impl EvmStateProvider for Provider {
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        self.calls.lock().unwrap().push(Call::Account(*address));
        if *address == Address::with_last_byte(0xee) {
            return Err(ProviderError::BlockHashNotFound(B256::ZERO))
        }
        Ok((*address != Address::with_last_byte(0xff)).then(Account::default))
    }

    fn block_hash(&self, _: u64) -> ProviderResult<Option<B256>> {
        Ok(None)
    }

    fn bytecode_by_hash(&self, _: &B256) -> ProviderResult<Option<ProviderBytecode>> {
        Ok(None)
    }

    fn storage(&self, address: Address, slot: StorageKey) -> ProviderResult<Option<U256>> {
        let slot = U256::from_be_bytes(slot.0);
        self.calls.lock().unwrap().push(Call::Storage(address, slot));
        if slot == U256::MAX {
            return Err(ProviderError::BlockHashNotFound(B256::ZERO))
        }
        Ok(Some(self.base + slot))
    }
}

fn generation() -> Arc<ViewGeneration> {
    Arc::new(ViewGeneration {
        live: AtomicBool::new(true),
        _payload_hash: B256::ZERO,
        _parent_hash: B256::ZERO,
        _parent_state_root: B256::ZERO,
    })
}

fn database(
    generation: Option<Arc<ViewGeneration>>,
    base: u64,
) -> (ParentDatabase, Arc<Mutex<Vec<Call>>>, std::sync::Weak<()>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let lifetime = Arc::new(());
    let weak = Arc::downgrade(&lifetime);
    let mut database = ParentDatabase::unbound(Box::new(Provider {
        calls: Arc::clone(&calls),
        base: U256::from(base),
        _lifetime: lifetime,
    }));
    database.generation = generation;
    (database, calls, weak)
}

fn batch(
    generation: Arc<ViewGeneration>,
    address: Address,
    slot: U256,
    index: usize,
) -> ParentReadBatch {
    let (mut worker, _, _) = database(Some(generation), 100);
    let hooks = ParentDatabase::capture_hooks();
    (hooks.begin)(&mut worker);
    assert_eq!((hooks.storage)(&mut worker, index, address, slot).unwrap(), U256::from(100) + slot);
    (hooks.finish)(&mut worker).unwrap()
}

#[test]
fn worker_reads_once_and_records_sparse_indices_with_duplicate_keys() {
    let (mut db, calls, _) = database(Some(generation()), 100);
    let hooks = ParentDatabase::capture_hooks();
    let address = Address::with_last_byte(1);
    (hooks.begin)(&mut db);
    for (index, slot) in [(2, 3), (9, 3), (15, 7)] {
        assert_eq!(
            (hooks.storage)(&mut db, index, address, U256::from(slot)).unwrap(),
            U256::from(100 + slot)
        );
    }
    let batch = (hooks.finish)(&mut db).unwrap();
    let reads = &batch.opaque.as_ref().downcast_ref::<StorageBatch>().unwrap().reads;
    assert_eq!(reads.iter().map(|read| read.index).collect::<Vec<_>>(), vec![2, 9, 15]);
    assert_eq!(
        *calls.lock().unwrap(),
        vec![
            Call::Storage(address, U256::from(3)),
            Call::Storage(address, U256::from(3)),
            Call::Storage(address, U256::from(7)),
        ]
    );
    assert_eq!(batch.estimated_bytes, BATCH_OVERHEAD + reads.capacity() * size_of::<StorageRead>());
    assert!(batch.estimated_bytes <= MAX_BATCH_BYTES);
    assert!((hooks.finish)(&mut db).is_none());
}

#[test]
fn failed_discarded_and_restarted_captures_do_not_publish_partial_reads() {
    let (mut db, calls, _) = database(Some(generation()), 100);
    let hooks = ParentDatabase::capture_hooks();
    let address = Address::with_last_byte(1);
    (hooks.begin)(&mut db);
    (hooks.storage)(&mut db, 0, address, U256::ZERO).unwrap();
    assert!((hooks.storage)(&mut db, 1, address, U256::MAX).is_err());
    assert!((hooks.finish)(&mut db).is_none());
    assert_eq!(calls.lock().unwrap().len(), 2);
    (hooks.begin)(&mut db);
    (hooks.storage)(&mut db, 0, address, U256::ZERO).unwrap();
    (hooks.discard)(&mut db);
    assert!((hooks.finish)(&mut db).is_none());
    (hooks.begin)(&mut db);
    (hooks.storage)(&mut db, 0, address, U256::ZERO).unwrap();
    (hooks.begin)(&mut db);
    (hooks.storage)(&mut db, 8, address, U256::from(8)).unwrap();
    let batch = (hooks.finish)(&mut db).unwrap();
    let reads = &batch.opaque.as_ref().downcast_ref::<StorageBatch>().unwrap().reads;
    assert_eq!(reads.len(), 1);
    assert_eq!(reads[0].index, 8);
    (hooks.begin)(&mut db);
    (hooks.storage)(&mut db, 5, address, U256::ZERO).unwrap();
    (hooks.storage)(&mut db, 5, address, U256::ZERO).unwrap();
    assert!(
        (hooks.finish)(&mut db).is_none(),
        "ambiguous read positions decline witness retention"
    );
}

#[test]
fn mismatched_opaque_view_position_key_value_and_closed_generation_fall_back() {
    let view = generation();
    let address = Address::with_last_byte(1);
    let slot = U256::from(3);
    let expected = U256::from(103);
    let valid = batch(Arc::clone(&view), address, slot, 4);
    let foreign = batch(generation(), address, slot, 4);
    let forged = ParentReadBatch { opaque: Arc::new(()), estimated_bytes: 0 };
    let (mut db, calls, _) = database(Some(Arc::clone(&view)), 900);
    db.certified_reads = Some(0);
    for (witness, index, key_address, key_slot, value) in [
        (&forged, 4, address, slot, expected),
        (&foreign, 4, address, slot, expected),
        (&valid, 5, address, slot, expected),
        (&valid, 4, Address::ZERO, slot, expected),
        (&valid, 4, address, U256::ZERO, expected),
        (&valid, 4, address, slot, U256::ZERO),
    ] {
        db.offer(witness, index, key_address, key_slot, value);
        assert_eq!(db.storage(address, slot).unwrap(), U256::from(903));
    }
    assert_eq!(calls.lock().unwrap().len(), 6);
    assert_eq!(db.certified_reads(), Some(0));
    db.offer(&valid, 4, address, slot, expected);
    assert_eq!(db.storage(address, slot).unwrap(), expected);
    assert_eq!(calls.lock().unwrap().len(), 6);
    assert_eq!(db.certified_reads(), Some(1));
    assert_eq!(db.storage(address, slot).unwrap(), U256::from(903), "offers are consumed once");
    db.offer(&valid, 4, address, slot, expected);
    view.live.store(false, Ordering::Release);
    assert_eq!(
        db.storage(address, slot).unwrap(),
        U256::from(903),
        "closure after offer is checked at use"
    );
    db.offer(&valid, 4, address, slot, expected);
    assert_eq!(db.storage(address, slot).unwrap(), U256::from(903));
    assert_eq!(db.certified_reads(), Some(1));
}

#[test]
fn ordinary_state_reads_materialize_and_observe_direct_cache_mutation() {
    let view = generation();
    let address = Address::with_last_byte(1);
    let slot = U256::from(3);
    let expected = U256::from(103);
    let witness = batch(Arc::clone(&view), address, slot, 4);
    let (db, calls, _) = database(Some(view), 100);
    let mut state = State::builder().with_database(db).build();
    (ParentDatabase::validation_hooks().offer)(
        &mut &mut state,
        &witness,
        4,
        address,
        slot,
        expected,
    );
    assert_eq!(state.storage(address, slot).unwrap(), expected);
    (ParentDatabase::validation_hooks().clear)(&mut &mut state);
    assert_eq!(*calls.lock().unwrap(), vec![Call::Account(address)]);
    assert_eq!(state.cache.accounts[&address].account.as_ref().unwrap().storage[&slot], expected);

    state
        .cache
        .accounts
        .get_mut(&address)
        .unwrap()
        .account
        .as_mut()
        .unwrap()
        .storage
        .insert(slot, U256::from(999));
    (ParentDatabase::validation_hooks().offer)(
        &mut &mut state,
        &witness,
        4,
        address,
        slot,
        expected,
    );
    assert_eq!(state.storage(address, slot).unwrap(), U256::from(999));
    (ParentDatabase::validation_hooks().clear)(&mut &mut state);
    assert!(state.database.offered.is_none());

    state.cache.accounts.get_mut(&address).unwrap().account.as_mut().unwrap().storage.remove(&slot);
    (ParentDatabase::validation_hooks().offer)(
        &mut &mut state,
        &witness,
        4,
        address,
        slot,
        expected,
    );
    assert_eq!(state.storage(address, slot).unwrap(), expected);
    (ParentDatabase::validation_hooks().clear)(&mut &mut state);
    assert_eq!(*calls.lock().unwrap(), vec![Call::Account(address)]);

    let (foreign, foreign_calls, _) = database(Some(generation()), 900);
    state.database = foreign;
    state.cache.accounts.get_mut(&address).unwrap().account.as_mut().unwrap().storage.remove(&slot);
    (ParentDatabase::validation_hooks().offer)(
        &mut &mut state,
        &witness,
        4,
        address,
        slot,
        expected,
    );
    assert_eq!(state.storage(address, slot).unwrap(), U256::from(903));
    (ParentDatabase::validation_hooks().clear)(&mut &mut state);
    assert_eq!(*foreign_calls.lock().unwrap(), vec![Call::Storage(address, slot)]);
}

#[test]
fn account_errors_nonexistence_and_clear_do_not_leak_offers() {
    let view = generation();
    let slot = U256::from(3);
    let bad_address = Address::with_last_byte(0xee);
    let witness = batch(Arc::clone(&view), bad_address, slot, 0);
    let (db, calls, _) = database(Some(Arc::clone(&view)), 100);
    let mut state = State::builder().with_database(db).build();
    (ParentDatabase::validation_hooks().offer)(
        &mut &mut state,
        &witness,
        0,
        bad_address,
        slot,
        U256::from(103),
    );
    assert!(state.storage(bad_address, slot).is_err());
    (ParentDatabase::validation_hooks().clear)(&mut &mut state);
    assert!(state.database.offered.is_none());
    assert_eq!(*calls.lock().unwrap(), vec![Call::Account(bad_address)]);

    let missing = Address::with_last_byte(0xff);
    let witness = batch(view, missing, slot, 0);
    (ParentDatabase::validation_hooks().offer)(
        &mut &mut state,
        &witness,
        0,
        missing,
        slot,
        U256::from(103),
    );
    assert_eq!(state.storage(missing, slot).unwrap(), U256::ZERO);
    (ParentDatabase::validation_hooks().clear)(&mut &mut state);
    assert!(state.database.offered.is_none());
    assert_eq!(*calls.lock().unwrap(), vec![Call::Account(bad_address), Call::Account(missing)]);
    assert!(state.storage(Address::ZERO, U256::MAX).is_err());
    assert_eq!(calls.lock().unwrap().last(), Some(&Call::Storage(Address::ZERO, U256::MAX)));
}

#[test]
fn unbound_and_bundle_states_keep_normal_reads_and_cannot_capture() {
    let view = generation();
    let address = Address::with_last_byte(1);
    let slot = U256::from(3);
    let witness = batch(Arc::clone(&view), address, slot, 0);
    let (mut db, calls, _) = database(None, 900);
    let capture = ParentDatabase::capture_hooks();
    (capture.begin)(&mut db);
    assert_eq!((capture.storage)(&mut db, 0, address, slot).unwrap(), U256::from(903));
    assert!((capture.finish)(&mut db).is_none());
    db.offer(&witness, 0, address, slot, U256::from(103));
    assert_eq!(db.storage(address, slot).unwrap(), U256::from(903));
    assert_eq!(calls.lock().unwrap().len(), 2);

    let (db, calls, _) = database(Some(view), 100);
    let mut state = State::builder().with_database(db).build();
    state.use_preloaded_bundle = true;
    (ParentDatabase::validation_hooks().offer)(
        &mut &mut state,
        &witness,
        0,
        address,
        slot,
        U256::from(103),
    );
    assert!(state.database.offered.is_none());
    assert_eq!(state.storage(address, slot).unwrap(), U256::from(103));
    (ParentDatabase::validation_hooks().clear)(&mut &mut state);
    assert_eq!(*calls.lock().unwrap(), vec![Call::Account(address), Call::Storage(address, slot)]);
}

#[test]
fn completed_batch_does_not_keep_provider_alive_and_closure_prevents_publication() {
    let view = generation();
    let (mut db, _, lifetime) = database(Some(Arc::clone(&view)), 100);
    db.begin();
    db.capture_storage(0, Address::ZERO, U256::ZERO).unwrap();
    let witness = db.finish().unwrap();
    drop(db);
    assert!(lifetime.upgrade().is_none(), "certificate must not retain a provider");
    assert!(witness.opaque.as_ref().downcast_ref::<StorageBatch>().is_some());

    let (mut db, calls, _) = database(Some(Arc::clone(&view)), 100);
    db.begin();
    db.capture_storage(0, Address::ZERO, U256::ZERO).unwrap();
    view.live.store(false, Ordering::Release);
    db.capture_storage(1, Address::ZERO, U256::from(1)).unwrap();
    assert!(db.finish().is_none());
    assert_eq!(calls.lock().unwrap().len(), 2, "closed capture still performs ordinary reads");
}

#[test]
fn attaching_bal_declines_offers_and_preserves_bal_errors() {
    let view = generation();
    let address = Address::with_last_byte(1);
    let slot = U256::from(3);
    let witness = batch(Arc::clone(&view), address, slot, 0);
    let (db, calls, _) = database(Some(view), 100);
    let mut state = State::builder().with_database(db).build();
    state.set_bal(Some(Arc::new(Bal::default())));
    (ParentDatabase::validation_hooks().offer)(
        &mut &mut state,
        &witness,
        0,
        address,
        slot,
        U256::from(103),
    );
    assert!(state.database.offered.is_none());
    assert!(state.storage(address, slot).is_err());
    (ParentDatabase::validation_hooks().clear)(&mut &mut state);
    assert!(calls.lock().unwrap().is_empty());
    state.set_allow_bal_db_fallback(true);
    (ParentDatabase::validation_hooks().offer)(
        &mut &mut state,
        &witness,
        0,
        address,
        slot,
        U256::from(103),
    );
    assert!(state.database.offered.is_none());
    assert_eq!(state.storage(address, slot).unwrap(), U256::from(103));
    (ParentDatabase::validation_hooks().clear)(&mut &mut state);
    assert_eq!(*calls.lock().unwrap(), vec![Call::Account(address), Call::Storage(address, slot)]);
}

#[test]
fn view_owns_actual_cache_only_until_closed_and_dropped() {
    let parent = B256::with_last_byte(1);
    let cache = SavedCache::new(parent, reth_execution_cache::ExecutionCache::new(1_000));
    let factory = OverlayStateProviderFactory::<_, EthPrimitives>::new(
        MockEthProvider::default(),
        OverlayManager::default().overlay_builder(parent),
    );
    assert!(ParentReadView::new(
        factory.clone(),
        cache.clone(),
        None,
        B256::ZERO,
        B256::ZERO,
        B256::ZERO
    )
    .is_none());
    let view = ParentReadView::new(
        factory,
        cache.clone(),
        None,
        B256::with_last_byte(2),
        parent,
        B256::with_last_byte(3),
    )
    .unwrap();
    assert_eq!(cache.usage_count(), 2);
    let witness = batch(Arc::clone(&view.generation), Address::ZERO, U256::ZERO, 0);
    let weak = Arc::downgrade(&view);
    view.close();
    drop(view);
    assert!(weak.upgrade().is_none());
    assert_eq!(cache.usage_count(), 1, "retained batch must not pin the selected cache");
    let batch = witness.opaque.as_ref().downcast_ref::<StorageBatch>().unwrap();
    assert!(!batch.generation.live.load(Ordering::Acquire));
}
