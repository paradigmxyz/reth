//! Continuous differential campaign for trie witnesses.

use alloy_primitives::{keccak256, map::HashMap, Address, Bytes, B256, U256};
use alloy_trie::proof::{verify_proof, ProofRetainer};
use rand::{rngs::StdRng, RngCore, SeedableRng};
use reth_db::{cursor::DbCursorRW, tables};
use reth_db_api::transaction::DbTxMut;
use reth_primitives_traits::{Account, StorageEntry};
use reth_provider::{test_utils::create_test_provider_factory, HashingWriter};
use reth_storage_api::StorageSettingsCache;
use reth_trie::{
    witness::TrieWitness, DecodedMultiProofV2, HashBuilder, HashedPostState, HashedStorage,
    Nibbles, StateRoot,
};
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseStateRoot, DatabaseTrieCursorFactory};
use reth_trie_sparse::{SparseStateTrie, TrieNodeEpoch};
use std::{collections::BTreeMap, env, process};

type DbStateRoot<'a, TX, A> =
    StateRoot<DatabaseTrieCursorFactory<&'a TX, A>, DatabaseHashedCursorFactory<&'a TX>>;

#[derive(Clone, Copy, Debug)]
enum TrieAction {
    Put { key: B256, value: U256 },
    Delete { key: B256 },
}

fn main() {
    let start_seed = env_u64("RETH_DST_SEED", 0);
    let cases = env_u64("RETH_DST_CASES", 1_000);
    let steps = env_u64("RETH_DST_STEPS", 64) as usize;

    for seed in start_seed..start_seed.saturating_add(cases) {
        if let Err(error) = run_case(seed, steps) {
            eprintln!("BUG witness seed={seed} steps={steps}: {error}");
            process::exit(1);
        }
        println!("witness seed={seed} steps={steps}");
    }
}

fn run_case(seed: u64, steps: usize) -> Result<(), String> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut state = BTreeMap::<B256, U256>::new();
    let mut actions = Vec::with_capacity(steps);

    for _ in 0..steps {
        let action = generate_action(&mut rng, &state);
        match action {
            TrieAction::Put { key, value } => {
                state.insert(key, value);
            }
            TrieAction::Delete { key } => {
                state.remove(&key);
            }
        }
        actions.push(action);
    }
    if state.is_empty() {
        let key = random_key(&mut rng);
        state.insert(key, U256::from(1));
        actions.push(TrieAction::Put { key, value: U256::from(1) });
    }

    let oracle = env::var("RETH_DST_ORACLE").unwrap_or_else(|_| "all".to_string());
    if oracle != "proof" {
        verify_witness_roundtrip(&state, &actions)?;
    }
    if oracle != "witness" {
        verify_adversarial_proofs(seed, &mut rng, &state, &actions)?;
    }
    Ok(())
}

fn verify_witness_roundtrip(
    state: &BTreeMap<B256, U256>,
    actions: &[TrieAction],
) -> Result<(), String> {
    let factory = create_test_provider_factory();
    let provider = factory.provider_rw().map_err(|error| error.to_string())?;
    let address = Address::ZERO;
    let hashed_address = keccak256(address);
    provider
        .insert_account_for_hashing([(address, Some(Account::default()))])
        .map_err(|error| error.to_string())?;
    let mut cursor = provider
        .tx_ref()
        .cursor_dup_write::<tables::HashedStorages>()
        .map_err(|error| error.to_string())?;
    for (&key, &value) in state {
        cursor
            .upsert(hashed_address, &StorageEntry { key, value })
            .map_err(|error| error.to_string())?;
    }
    drop(cursor);

    reth_trie_db::with_adapter!(provider, |A| {
        let root = DbStateRoot::<_, A>::from_tx(provider.tx_ref())
            .root()
            .map_err(|error| error.to_string())?;
        let transition = HashedPostState {
            accounts: HashMap::from_iter([(hashed_address, Some(Account::default()))]),
            storages: HashMap::from_iter([(
                hashed_address,
                HashedStorage::from_iter(state.iter().map(|(&key, &value)| (key, value))),
            )]),
        };
        let witness = TrieWitness::new(
            DatabaseTrieCursorFactory::<_, A>::new(provider.tx_ref()),
            DatabaseHashedCursorFactory::new(provider.tx_ref()),
        )
        .compute(transition)
        .map_err(|error| error.to_string())?;
        let actual =
            DecodedMultiProofV2::from_witness(root, &witness).map_err(|error| error.to_string())?;
        let proof_nodes = actual.account_proofs.len() +
            actual.storage_proofs.values().map(Vec::len).sum::<usize>();
        let mut sparse = SparseStateTrie::new();
        sparse
            .reveal_decoded_multiproof_v2(actual)
            .map_err(|error| format!("reconstructed witness could not be revealed: {error}"))?;
        for (&key, &value) in state {
            let expected = alloy_rlp::encode_fixed_size(&value).to_vec();
            if !sparse.check_valid_storage_witness(hashed_address, key) ||
                sparse.get_storage_slot_value(&hashed_address, &key) != Some(&expected)
            {
                return Err(format!(
                    "witness omitted or changed reachable storage target={key} root={root} entries={} proof_nodes={proof_nodes} actions={actions:?}",
                    witness.len(),
                ));
            }
        }
        let reconstructed_root = sparse
            .root(TrieNodeEpoch::UNMODIFIED)
            .map_err(|error| format!("reconstructed witness root failed: {error}"))?;
        if reconstructed_root != root {
            return Err(format!(
                "witness root mismatch expected={root} actual={reconstructed_root} entries={} proof_nodes={proof_nodes} actions={actions:?}",
                witness.len(),
            ));
        }
        Ok::<(), String>(())
    })
}

fn verify_adversarial_proofs(
    seed: u64,
    rng: &mut StdRng,
    state: &BTreeMap<B256, U256>,
    actions: &[TrieAction],
) -> Result<(), String> {
    let target = choose_key(rng, state);
    let target_path = Nibbles::unpack(target);
    let mut builder = HashBuilder::default()
        .with_proof_retainer(ProofRetainer::new(std::iter::once(target_path).collect()));
    for (&key, &value) in state {
        builder.add_leaf(Nibbles::unpack(key), &value.to_be_bytes::<32>());
    }
    let root = builder.root();
    let proof = builder
        .take_proof_nodes()
        .into_nodes_sorted()
        .into_iter()
        .map(|(_, node)| node)
        .collect::<Vec<Bytes>>();
    let expected = state[&target].to_be_bytes::<32>().to_vec();
    verify_proof(root, target_path, Some(expected.clone()), &proof)
        .map_err(|error| format!("honest proof rejected seed={seed}: {error}"))?;

    // Soundness is checked against the authoritative map. Mutated proofs may remain valid, but
    // they must never authenticate a value that differs from the model.
    for end in 0..proof.len() {
        if verify_proof(root, target_path, None, &proof[..end]).is_ok() {
            return Err(format!(
                "invalid exclusion accepted mutation=truncate end={end}/{} target={target} actions={actions:?}",
                proof.len(),
            ));
        }
    }

    let mut wrong_value = vec![0u8; expected.len().max(1)];
    rng.fill_bytes(&mut wrong_value);
    if wrong_value == expected {
        wrong_value[0] ^= 1;
    }
    let mutations = mutate_proof(rng, &proof);
    for (kind, mutated) in mutations {
        if verify_proof(root, target_path, Some(wrong_value.clone()), &mutated).is_ok() {
            return Err(format!(
                "incorrect value accepted mutation={kind} target={target} actions={actions:?}"
            ));
        }
        if verify_proof(root, target_path, None, &mutated).is_ok() {
            return Err(format!(
                "invalid exclusion accepted mutation={kind} target={target} actions={actions:?}"
            ));
        }
    }
    Ok(())
}

fn mutate_proof(rng: &mut StdRng, proof: &[Bytes]) -> Vec<(&'static str, Vec<Bytes>)> {
    let mut mutations = Vec::new();
    if let Some(last) = proof.last() {
        let mut appended = proof.to_vec();
        appended.push(last.clone());
        mutations.push(("append", appended));
    }
    if proof.len() > 1 {
        let mut swapped = proof.to_vec();
        let index = rng.next_u64() as usize % (swapped.len() - 1);
        swapped.swap(index, index + 1);
        mutations.push(("reorder", swapped));

        let mut duplicated = proof.to_vec();
        duplicated.insert(index, duplicated[index].clone());
        mutations.push(("duplicate", duplicated));
    }
    if !proof.is_empty() {
        let mut flipped = proof.to_vec();
        let node = rng.next_u64() as usize % flipped.len();
        if !flipped[node].is_empty() {
            let byte = rng.next_u64() as usize % flipped[node].len();
            let mut bytes = flipped[node].to_vec();
            bytes[byte] ^= 1u8 << (rng.next_u64() % 8);
            flipped[node] = Bytes::from(bytes);
            mutations.push(("bit-flip", flipped));
        }
    }
    mutations
}

fn generate_action(rng: &mut StdRng, state: &BTreeMap<B256, U256>) -> TrieAction {
    if !state.is_empty() && rng.next_u64() & 7 == 0 {
        let key = choose_key(rng, state);
        return TrieAction::Delete { key }
    }

    let key = if !state.is_empty() && rng.next_u64() & 3 != 0 {
        let mut key = choose_key(rng, state).0;
        // Local mutations explore nearby trie paths while unconstrained keys keep global coverage.
        let offset = 31 - (rng.next_u64().trailing_zeros() as usize).min(31);
        key[offset] ^= 1u8 << (rng.next_u64() % 8);
        B256::from(key)
    } else {
        random_key(rng)
    };
    let value = match rng.next_u64() % 4 {
        0 => U256::from(rng.next_u64() & 0xff),
        1 => U256::from(rng.next_u64() & 0xffff),
        _ => U256::from(rng.next_u64()),
    };
    TrieAction::Put { key, value: value.max(U256::from(1)) }
}

fn choose_key(rng: &mut StdRng, state: &BTreeMap<B256, U256>) -> B256 {
    let index = rng.next_u64() as usize % state.len();
    *state.keys().nth(index).expect("state is non-empty")
}

fn random_key(rng: &mut StdRng) -> B256 {
    let mut key = [0u8; 32];
    rng.fill_bytes(&mut key);
    B256::from(key)
}

fn env_u64(name: &str, default: u64) -> u64 {
    env::var(name).ok().and_then(|value| value.parse().ok()).unwrap_or(default)
}
