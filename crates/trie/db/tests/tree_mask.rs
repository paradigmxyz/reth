#![allow(missing_docs)]

use alloy_primitives::{B256, U256};
use reth_db::{cursor::DbCursorRW, tables, transaction::DbTxMut};
use reth_primitives::Account;
use reth_provider::test_utils::create_test_provider_factory;
use reth_trie::{trie_cursor::InMemoryTrieCursorFactory, HashedPostState, StateRoot};
use reth_trie_db::{DatabaseStateRoot, DatabaseTrieCursorFactory};
use std::{collections::BTreeMap, str::FromStr};

mod common;
use common::verify_tree_mask_invariant;

#[test]
fn test_tree_mask_invariant_regression() {
    let factory = create_test_provider_factory();
    let provider = factory.provider_rw().unwrap();
    let mut hashed_account_cursor =
        provider.tx_ref().cursor_write::<tables::HashedAccounts>().unwrap();

    // initial state that triggered the bug
    let mut init_state = BTreeMap::new();
    init_state.insert(
        B256::from_str("b0eadc24c8b856c298f120d53504054937ad6f10981f6cfd4f7dd28b488de6bb").unwrap(),
        U256::from_str(
            "8191087236450936987389588093826784753915004608762462187916580788651909579970",
        )
        .unwrap(),
    );
    init_state.insert(
        B256::from_str("b650000000000000000000000000000000000000000000000000000000000000").unwrap(),
        U256::from_str("1502831726292624754819496855058931322619830474306082077188255614238720")
            .unwrap(),
    );
    init_state.insert(
        B256::from_str("b65217bdbbccae258ec1b7ddfbc73abd91693a291c9ee2df9756408f4e097d7f").unwrap(),
        U256::from_str(
            "67678148959447529652628808732627110556888940958577363264860875821424387015141",
        )
        .unwrap(),
    );
    init_state.insert(
        B256::from_str("b652c6105e18b957ed216d5dabe509dc9b87060e1cc3da097e96b60859a473fb").unwrap(),
        U256::from_str(
            "112824820931552671541607645878429951093128233648894736867125614595897428715080",
        )
        .unwrap(),
    );

    // insert init state into database
    for (hashed_address, balance) in init_state.clone() {
        hashed_account_cursor
            .upsert(hashed_address, Account { balance, ..Default::default() })
            .unwrap();
    }

    // get initial root and updates
    let (_, mut trie_nodes) = StateRoot::from_tx(provider.tx_ref()).root_with_updates().unwrap();

    // verify initial tree mask invariant
    assert!(verify_tree_mask_invariant(
        trie_nodes.account_nodes_ref(),
        trie_nodes.removed_nodes_ref()
    ));

    // first update from the failing test case
    let update = BTreeMap::from([
        (
            B256::from_str("245edb422018381f402c25b6cd6510b23adfb89e8aae077e2267dd1e38d65b82")
                .unwrap(),
            Some(
                U256::from_str(
                    "32389638575033086484031574202531845846535608532580941050116309514422859555371",
                )
                .unwrap(),
            ),
        ),
        (
            B256::from_str("785021a08d7111e76c17897b02df917dd86b381260b51bf80f5b868e3029e7c2")
                .unwrap(),
            Some(
                U256::from_str(
                    "19591547302947487246265266157960068499012679158449160478744628061815918167659",
                )
                .unwrap(),
            ),
        ),
        (
            B256::from_str("c30771ee925a78dec279b691233b7445ef826648fe8d95ffe49f563edc5cb713")
                .unwrap(),
            Some(
                U256::from_str(
                    "67939140076345108486057192637455821946901230687705312285974934873833035160833",
                )
                .unwrap(),
            ),
        ),
    ]);

    // apply update
    let mut hashed_state = HashedPostState::default();
    for (hashed_address, balance) in update {
        if let Some(balance) = balance {
            let account = Account { balance, ..Default::default() };
            hashed_account_cursor.upsert(hashed_address, account).unwrap();
            hashed_state.accounts.insert(hashed_address, Some(account));
        } else {
            hashed_state.accounts.insert(hashed_address, None);
        }
    }

    // calculate new root with in-memory trie nodes overlay
    let (_, trie_updates) = StateRoot::from_tx(provider.tx_ref())
        .with_prefix_sets(hashed_state.construct_prefix_sets().freeze())
        .with_trie_cursor_factory(InMemoryTrieCursorFactory::new(
            DatabaseTrieCursorFactory::new(provider.tx_ref()),
            &trie_nodes.clone().into_sorted(),
        ))
        .root_with_updates()
        .unwrap();

    trie_nodes.extend(trie_updates);

    assert!(verify_tree_mask_invariant(
        trie_nodes.account_nodes_ref(),
        trie_nodes.removed_nodes_ref()
    ));
}
