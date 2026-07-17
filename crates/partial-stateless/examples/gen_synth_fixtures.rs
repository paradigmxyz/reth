//! Dev helper: generate synthetic `AccessedStateFixture`s so the offline
//! `cache_window_bench` can be smoke-tested without a node.
//!
//!   cargo run -p partial-stateless --example gen_synth_fixtures -- <dir> <n_blocks>
//!
//! Each block re-touches a sliding set of "hot" accounts/slots plus a few fresh
//! "cold" ones, so larger cache windows produce visibly higher hit ratios.

use alloy_primitives::{Address, B256, U256};
use partial_stateless::{accessed_state::BlockAccessedState, fixture::save_fixture, policy::AccountData, AccessedStateFixture};
use std::path::PathBuf;

fn addr(n: u64) -> Address {
    let mut b = [0u8; 20];
    b[12..20].copy_from_slice(&n.to_be_bytes());
    Address::from(b)
}
fn slot(n: u64) -> B256 {
    B256::from(U256::from(n))
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().unwrap_or_else(|| "./fixtures/accessed".into()));
    let n: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(300);

    for block in 1..=n {
        let mut accessed = BlockAccessedState::default();
        // 40 "hot" accounts that recur every block (cacheable across any window).
        for i in 0..40 {
            let a = addr(i);
            accessed.accounts.insert(a, AccountData { nonce: block, balance: U256::from(block), code_hash: None });
            accessed.storage.insert((a, slot(i)), U256::from(block));
        }
        // 20 "warm" accounts on a ~50-block cycle (hit only by larger windows).
        for i in 0..20 {
            let a = addr(1000 + (block % 50) * 20 + i);
            accessed.accounts.insert(a, AccountData { nonce: 0, balance: U256::ZERO, code_hash: None });
            accessed.storage.insert((a, slot(i)), U256::from(i));
        }
        // 30 "cold" accounts unique to this block (always a miss).
        for i in 0..30 {
            let a = addr(1_000_000 + block * 100 + i);
            accessed.accounts.insert(a, AccountData { nonce: 0, balance: U256::ZERO, code_hash: None });
        }

        let fixture = AccessedStateFixture {
            block_number: block,
            block_hash: B256::from(U256::from(block)),
            parent_state_root: B256::ZERO,
            accessed,
        };
        save_fixture(&dir, &fixture).expect("save fixture");
    }
    println!("wrote {n} synthetic fixtures to {}", dir.display());
}
