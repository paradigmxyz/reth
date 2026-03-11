use alloy_primitives::{keccak256, Address, B256, U256};
use alloy_rlp::{Decodable, Encodable};
use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};
use reth_engine_tree::tree::payload_processor::sparse_trie::bench_promote_account_leaf_update;
use reth_primitives_traits::Account;
use reth_trie::{TrieAccount, EMPTY_ROOT_HASH, TRIE_ACCOUNT_RLP_MAX_SIZE};
use reth_trie_sparse::LeafUpdate;
use revm_primitives::B256Map;

const PROMOTION_ROUNDS: usize = 12;

#[derive(Clone)]
struct PromotionInput {
    address: B256,
    trie_account_rlp: Vec<u8>,
    pending_account: Option<Account>,
    override_storage_root: B256,
}

fn make_input(i: u64) -> PromotionInput {
    let address = keccak256(Address::with_last_byte(i as u8));
    let trie_storage_root =
        if i % 5 == 0 { EMPTY_ROOT_HASH } else { keccak256((i * 17).to_be_bytes()) };
    let trie_account = TrieAccount {
        nonce: i % 7,
        balance: U256::from(i * 13 + 1),
        storage_root: trie_storage_root,
        code_hash: if i % 3 == 0 { B256::ZERO } else { keccak256((i * 31).to_be_bytes()) },
    };

    let mut trie_account_rlp = Vec::with_capacity(TRIE_ACCOUNT_RLP_MAX_SIZE);
    trie_account.encode(&mut trie_account_rlp);

    let pending_account = if i % 4 == 0 {
        Some(Account {
            nonce: i % 11,
            balance: U256::from(i * 19 + 3),
            bytecode_hash: (i % 6 != 0).then(|| keccak256(Address::with_last_byte(i as u8))),
        })
    } else {
        None
    };

    let override_storage_root =
        if i % 8 == 0 { EMPTY_ROOT_HASH } else { keccak256((i * 23).to_be_bytes()) };

    PromotionInput { address, trie_account_rlp, pending_account, override_storage_root }
}

fn bench_sparse_trie_account_promotion(c: &mut Criterion) {
    let mut group = c.benchmark_group("sparse_trie_account_promotion");
    group.sample_size(30);

    for entries in [256usize, 1024, 4096] {
        let inputs: Vec<_> = (0..entries as u64).map(make_input).collect();

        group.bench_function(format!("mixed_entries/{entries}"), |b| {
            b.iter_batched(
                || {
                    (
                        inputs.clone(),
                        Vec::with_capacity(TRIE_ACCOUNT_RLP_MAX_SIZE),
                        B256Map::<LeafUpdate>::default(),
                    )
                },
                |(inputs, mut account_rlp_buf, mut account_updates)| {
                    for _ in 0..PROMOTION_ROUNDS {
                        for input in &inputs {
                            let trie_account =
                                TrieAccount::decode(&mut &input.trie_account_rlp[..])
                                    .expect("valid trie account");

                            let storage_root = if input.pending_account.is_some() {
                                trie_account.storage_root
                            } else {
                                input.override_storage_root
                            };

                            let account = input.pending_account.or_else(|| {
                                (trie_account.storage_root != EMPTY_ROOT_HASH ||
                                    trie_account.nonce != 0 ||
                                    trie_account.balance != U256::ZERO ||
                                    trie_account.code_hash != B256::ZERO)
                                    .then(|| trie_account.into())
                            });

                            bench_promote_account_leaf_update(
                                input.address,
                                account,
                                storage_root,
                                &mut account_rlp_buf,
                                &mut account_updates,
                            );
                        }
                    }

                    black_box(account_updates);
                },
                BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

criterion_group!(sparse_trie_account_promotion, bench_sparse_trie_account_promotion);
criterion_main!(sparse_trie_account_promotion);
