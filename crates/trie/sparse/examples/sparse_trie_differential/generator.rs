//! Stateful account, storage and clustered working-set generation on prehashed keys.

use super::{
    sparse_trie_accounts::{update_model, Model},
    Action, Change, Input, INPUT_VERSION,
};
use alloy_primitives::{B256, U256};
use rand::{rngs::StdRng, seq::SliceRandom, Rng, SeedableRng};
use std::collections::BTreeSet;

const ROUNDS: usize = 16;
const UPDATES: usize = 4;

pub(super) fn generate(seed: u64) -> Input {
    let mut rng = StdRng::seed_from_u64(seed ^ 0x636c_7573_7465_7200);
    // Retain plain histories and at least one normal episode in every mixed history.
    let (rounds, clustered) = if rng.random_ratio(1, 3) {
        (unstructured(seed, ROUNDS), None)
    } else {
        let length = rng.random_range(6..=ROUNDS - 2);
        let episode = clustered_episode(&mut rng, length);
        let mut rounds = unstructured(seed, ROUNDS - length);
        let at = rng.random_range(1..=rounds.len());
        rounds.splice(at..at, episode);
        (rounds, Some(at..at + length))
    };
    let mut present = BTreeSet::new();
    let valid = rounds.iter().flatten().all(|action| match action {
        Action::UpdateBatch { updates } => updates.iter().all(|change| match change {
            Change::Account { key, .. } => {
                present.insert(*key);
                true
            }
            Change::DeleteAccount { key } => {
                present.remove(key);
                true
            }
            Change::Storage { key, .. } => present.contains(key),
            Change::Leaf { .. } => unreachable!(),
        }),
        _ => true,
    });
    let (rounds, clustered) =
        if valid { (rounds, clustered) } else { (unstructured(seed, ROUNDS), None) };
    Input { version: INPUT_VERSION, fixture_seed: seed, clustered, rounds }
}

fn unstructured(seed: u64, rounds: usize) -> Vec<Vec<Action>> {
    let mut population_rng = StdRng::seed_from_u64(seed ^ 0x726f_7461_7465_0000);
    let mut population = BTreeSet::new();
    while population.len() < 256 {
        population.insert(B256::from(population_rng.random::<[u8; 32]>()));
    }
    let mut population = population.into_iter().collect::<Vec<_>>();
    population.shuffle(&mut population_rng);
    let mut introduced = population_rng.random_range(1..=population.len().min(32).min(UPDATES));
    let mut state = Model::new();
    let initialized = population[..introduced]
        .iter()
        .map(|key| Change::Account { key: key.0, nonce: 1, value: U256::ONE.to_be_bytes() })
        .collect::<Vec<_>>();
    for change in &initialized {
        update_model(&mut state, change);
    }
    let mut rounds_out = vec![vec![Action::UpdateBatch { updates: initialized }]];
    let mut hot = Vec::new();
    let mut remaining = 0;
    let mut account_weight = 0;
    let mut rotating = 0;
    for number in 1..rounds {
        if remaining == 0 {
            hot = population[..introduced].to_vec();
            hot.shuffle(&mut population_rng);
            hot.truncate(population_rng.random_range(1..=introduced.min(UPDATES).min(8)));
            let available = rounds - number;
            remaining = if population_rng.random_bool(0.5) {
                population_rng.random_range(1..=available.min(8))
            } else {
                population_rng.random_range(1..=available)
            };
            account_weight = population_rng.random_range(0..=4);
        }
        remaining -= 1;
        let mut active = Vec::new();
        if introduced < population.len() && population_rng.random_bool(0.25) {
            let count = population_rng
                .random_range(1..=(population.len() - introduced).min(UPDATES).min(8));
            active.extend_from_slice(&population[introduced..introduced + count]);
            introduced += count;
        }
        active.extend(hot.iter().copied().take(UPDATES - active.len()));
        let extra = population_rng
            .random_range(0..=introduced.min(UPDATES).min(16).saturating_sub(active.len()));
        let target = active.len() + extra;
        for _ in 0..introduced {
            if active.len() == target {
                break;
            }
            let key = population[rotating % introduced];
            rotating = (rotating + 1) % introduced;
            if !active.contains(&key) {
                active.push(key);
            }
        }
        let mut rng = StdRng::seed_from_u64(seed.wrapping_add(number as u64));
        let generated = generate_updates(&mut rng, &mut state, &active, account_weight);
        let mut targets = generated
            .iter()
            .map(|update| {
                let slot = match update {
                    Change::Storage { slot, .. } => Some(B256::from(*slot)),
                    _ => None,
                };
                (B256::from(update.key()), slot)
            })
            .collect::<Vec<_>>();
        targets.shuffle(&mut rng);
        targets.truncate(rng.random_range(1..targets.len()));
        // Bound the wider account/slot hints so the worst case fits the native request.
        // This hint deliberately misses the population, including future introductions.
        let mut key = B256::from(rng.random::<[u8; 32]>());
        while population.contains(&key) || targets.iter().any(|(target, _)| *target == key) {
            key = B256::from(rng.random::<[u8; 32]>());
        }
        targets.push((key, None));
        targets.shuffle(&mut rng);
        let mut targets = targets.into_iter().peekable();
        let mut updates = generated.into_iter().peekable();
        let mut actions = Vec::new();
        while targets.peek().is_some() || updates.peek().is_some() {
            if targets.peek().is_some() && (updates.peek().is_none() || rng.random_bool(0.5)) {
                let size = rng.random_range(1..=targets.len().min(16));
                actions.extend(
                    targets
                        .by_ref()
                        .take(size)
                        .map(|(key, slot)| Action::Prewarm { key: key.0, slot }),
                );
            } else {
                let size = rng.random_range(1..=updates.len().min(16));
                actions
                    .push(Action::UpdateBatch { updates: updates.by_ref().take(size).collect() });
            }
        }
        rounds_out.push(actions);
    }
    rounds_out
}

fn clustered_episode(rng: &mut StdRng, rounds: usize) -> Vec<Vec<Action>> {
    let leaves = clustered_keys(rng);
    let targets = clustered_targets(rng, leaves, rounds);
    let mut state = Model::new();
    targets
        .into_iter()
        .map(|active| {
            let generated = generate_updates(rng, &mut state, &active, 4);
            let mut actions = Vec::new();
            if rng.random_bool(0.5) {
                actions.extend(
                    active.into_iter().map(|key| Action::Prewarm { key: key.0, slot: None }),
                );
            }
            actions.push(Action::UpdateBatch { updates: generated });
            actions
        })
        .collect()
}

fn clustered_targets(rng: &mut StdRng, mut leaves: Vec<B256>, rounds: usize) -> Vec<Vec<B256>> {
    leaves.shuffle(rng);
    let mut introduced = 0;
    let mut active = Vec::new();
    let mut targets = Vec::new();
    for _ in 0..rounds {
        if introduced == 0 || rng.random_bool(0.25) {
            introduced += rng
                .random_range(0..=leaves.len() - introduced)
                .max(1)
                .min(leaves.len() - introduced);
        }
        if active.is_empty() || rng.random_bool(0.25) {
            active = leaves[..introduced].to_vec();
            active.shuffle(rng);
            active.truncate(rng.random_range(1..=active.len().min(UPDATES)));
        }
        targets.push(active.clone());
    }
    targets
}

fn clustered_keys(rng: &mut StdRng) -> Vec<B256> {
    // Six nibbles reproduce clustered_indices(bits = 24) without a preimage table.
    let digits = 6u8;
    let shared = rng.random_range(0..=digits - 3);
    let count = rng.random_range(6..=24);
    let mut prefixes = vec![rng.random_range(0..1u64 << 24)];
    while prefixes.len() < count {
        let anchor = prefixes[rng.random_range(0..prefixes.len())];
        let suffix_bits = 4 * rng.random_range(1..=digits - shared);
        let mask = (1u64 << suffix_bits) - 1;
        let prefix = (anchor & !mask) | rng.random_range(0..=mask);
        if !prefixes.contains(&prefix) {
            prefixes.push(prefix);
        }
    }
    prefixes.shuffle(rng);
    prefixes
        .into_iter()
        .map(|prefix| {
            // A 24-bit prefix occupies three bytes; the other 29 bytes remain random.
            let mut key = rng.random::<[u8; 32]>();
            key[..3].copy_from_slice(&prefix.to_be_bytes()[5..]);
            B256::from(key)
        })
        .collect()
}

fn generate_updates(
    rng: &mut StdRng,
    known: &mut Model,
    active: &[B256],
    rewrite_weight: u32,
) -> Vec<Change> {
    (0..UPDATES)
        .map(|index| {
            let key = active
                [if index < active.len() { index } else { rng.random_range(0..active.len()) }];
            // Account removal precedes metadata/storage weighting, including clustered episodes.
            let change = if known.contains_key(&key) && rng.random_ratio(1, 8) {
                Change::DeleteAccount { key: key.0 }
            } else if !known.contains_key(&key) || rng.random_ratio(rewrite_weight, 4) {
                Change::Account {
                    key: key.0,
                    nonce: known.get(&key).map_or(1, |account| account.nonce + 1),
                    value: U256::from(rng.random_range(1..=1_000_000u64)).to_be_bytes(),
                }
            } else {
                let touched = &known[&key].touched;
                let slot = if !touched.is_empty() && rng.random_bool(0.5) {
                    *touched
                        .iter()
                        .nth(rng.random_range(0..touched.len()))
                        .expect("bounded slot index")
                } else {
                    B256::from(rng.random::<[u8; 32]>())
                };
                let value = if rng.random_ratio(1, 3) {
                    U256::ZERO
                } else {
                    U256::from(rng.random::<u64>())
                };
                Change::Storage { key: key.0, slot: slot.0, value: value.to_be_bytes() }
            };
            update_model(known, &change);
            change
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn generated_histories_include_clustered_deletions_and_storage_wipes() {
        let mut deleted = false;
        let mut recreated = false;
        let mut wiped = false;
        let mut clustered_delete = false;
        for seed in 0..128 {
            let input = generate(seed);
            assert_eq!(input, generate(seed));
            input.validate().unwrap();
            let mut state = Model::new();
            let mut removed = BTreeSet::new();
            for change in input.rounds.iter().flatten().flat_map(|action| match action {
                Action::UpdateBatch { updates } => updates.as_slice(),
                _ => &[],
            }) {
                let key = B256::from(change.key());
                match change {
                    Change::DeleteAccount { .. } => {
                        deleted = true;
                        wiped |= state.get(&key).is_some_and(|account| !account.storage.is_empty());
                        removed.insert(key);
                    }
                    Change::Account { .. } => recreated |= removed.contains(&key),
                    Change::Storage { .. } => assert!(state.contains_key(&key)),
                    Change::Leaf { .. } => unreachable!(),
                }
                update_model(&mut state, change);
            }
            let mut rng = StdRng::seed_from_u64(seed);
            clustered_delete |= clustered_episode(&mut rng, 14).iter().flatten().any(|action|
                matches!(action, Action::UpdateBatch { updates } if updates.iter().any(|change| matches!(change, Change::DeleteAccount { .. }))));
        }
        assert!(deleted && recreated && wiped && clustered_delete);
    }

    #[test]
    fn clustered_prefixes_vary_geometry() {
        let mut counts = BTreeSet::new();
        let mut fanouts = BTreeSet::new();
        let mut depths = BTreeSet::new();
        let mut densities = BTreeSet::new();
        for seed in 0..128 {
            let leaves = clustered_keys(&mut StdRng::seed_from_u64(seed));
            assert_eq!(leaves, clustered_keys(&mut StdRng::seed_from_u64(seed)));
            let prefixes = leaves
                .iter()
                .map(|key| u32::from_be_bytes([0, key[0], key[1], key[2]]))
                .collect::<BTreeSet<_>>();
            assert_eq!(prefixes.len(), leaves.len());
            assert!((6..=24).contains(&prefixes.len()));
            counts.insert(prefixes.len());
            let mut branches = BTreeMap::<(usize, u32), BTreeMap<u32, usize>>::new();
            for prefix in prefixes {
                for depth in 0..6 {
                    let parent = prefix >> (4 * (6 - depth));
                    let child = (prefix >> (4 * (5 - depth))) & 15;
                    *branches.entry((depth, parent)).or_default().entry(child).or_default() += 1;
                }
            }
            for ((depth, _), children) in branches {
                if children.len() > 1 {
                    fanouts.insert(children.len());
                    depths.insert(depth);
                    densities.extend(children.into_values());
                }
            }
        }
        assert!(counts.len() >= 12);
        assert!(fanouts.len() >= 4);
        assert_eq!(depths.len(), 6);
        assert!(densities.len() >= 8);
    }

    #[test]
    fn clustered_targets_persist_refresh_and_introduce() {
        let leaves = (1..=24).map(B256::with_last_byte).collect::<Vec<_>>();
        let mut repeated = false;
        let mut disjoint = false;
        let mut overlapping = false;
        let mut delayed = false;
        let mut sizes = BTreeSet::new();
        for seed in 0..128 {
            let targets = clustered_targets(&mut StdRng::seed_from_u64(seed), leaves.clone(), 14);
            assert_eq!(
                targets,
                clustered_targets(&mut StdRng::seed_from_u64(seed), leaves.clone(), 14)
            );
            assert_eq!(targets.len(), 14);
            let initial = targets[0].iter().copied().collect::<BTreeSet<_>>();
            for active in &targets {
                let set = active.iter().copied().collect::<BTreeSet<_>>();
                assert_eq!(set.len(), active.len());
                assert!(!active.is_empty() && active.len() <= UPDATES);
                assert!(active.iter().all(|key| leaves.contains(key)));
                delayed |= !set.is_subset(&initial);
                sizes.insert(active.len());
            }
            for pair in targets.windows(2) {
                let a = pair[0].iter().copied().collect::<BTreeSet<_>>();
                let b = pair[1].iter().copied().collect::<BTreeSet<_>>();
                repeated |= a == b;
                disjoint |= a.is_disjoint(&b);
                overlapping |= a != b && !a.is_disjoint(&b);
            }
        }
        assert!(repeated && disjoint && overlapping && delayed);
        assert_eq!(sizes, BTreeSet::from([1, 2, 3, 4]));
    }

    #[test]
    fn initialization_and_working_sets_stay_bounded() {
        let mut plain = false;
        let mut mixed = false;
        let mut sizes = BTreeSet::new();
        for seed in 0..128 {
            let input = generate(seed);
            plain |= input.clustered.is_none();
            mixed |= input.clustered.is_some();
            let Action::UpdateBatch { updates } = &input.rounds[0][0] else {
                panic!("initialization batch")
            };
            assert!((1..=UPDATES).contains(&updates.len()));
            assert!(updates
                .iter()
                .all(|change| matches!(change, Change::Account { nonce: 1, .. })));
            for actions in &input.rounds[1..] {
                let changes = actions
                    .iter()
                    .flat_map(|action| match action {
                        Action::UpdateBatch { updates } => updates.as_slice(),
                        _ => &[],
                    })
                    .collect::<Vec<_>>();
                assert_eq!(changes.len(), UPDATES);
                let active = changes.iter().map(|change| change.key()).collect::<BTreeSet<_>>();
                sizes.insert(active.len());
            }
        }
        assert!(plain && mixed);
        assert_eq!(sizes, BTreeSet::from([1, 2, 3, 4]));
    }

    #[test]
    fn normal_episodes_stage_rotate_persist_and_interleave() {
        let mut staged = false;
        let mut persisted = false;
        let mut rotated = false;
        let mut deletion = false;
        let mut reveal_before = false;
        let mut reveal_after = false;
        let mut sizes = BTreeSet::new();
        for seed in 0..128 {
            let rounds = unstructured(seed, ROUNDS);
            let mut known = updates(&rounds[0]).iter().map(|u| u.key()).collect::<BTreeSet<_>>();
            let mut previous = known.clone();
            for round in &rounds[1..] {
                let generated = updates(round);
                let active = generated.iter().map(|u| u.key()).collect::<BTreeSet<_>>();
                assert_eq!(generated.len(), UPDATES);
                assert!(!active.is_empty() && active.len() <= UPDATES);
                sizes.insert(active.len());
                staged |= !active.is_subset(&known);
                persisted |= !active.is_disjoint(&previous);
                rotated |= !previous.is_subset(&active);
                deletion |= generated.iter().any(|u| matches!(u, Change::DeleteAccount { .. }));
                let hints = round
                    .iter()
                    .filter_map(|action| match action {
                        Action::Prewarm { key, .. } => Some(*key),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                assert!((2..=4).contains(&hints.len()));
                assert!(hints.iter().any(|key| active.contains(key)));
                assert!(hints.iter().any(|key| !active.contains(key)));
                reveal_before |= matches!(round.first(), Some(Action::Prewarm { .. }));
                reveal_after |= matches!(round.last(), Some(Action::Prewarm { .. }));
                known.extend(&active);
                previous = active;
            }
        }
        assert!(staged && persisted && rotated && deletion);
        assert!(reveal_before && reveal_after);
        assert_eq!(sizes, BTreeSet::from([1, 2, 3, 4]));
    }

    #[test]
    fn mixed_histories_splice_whole_clustered_rounds_and_retain_normal_episodes() {
        let mut lengths = BTreeSet::new();
        let mut prewarmed = false;
        let mut cold = false;
        for seed in 0..128 {
            let mut rng = StdRng::seed_from_u64(seed ^ 0x636c_7573_7465_7200);
            if rng.random_ratio(1, 3) {
                assert_eq!(generate(seed).rounds, unstructured(seed, ROUNDS));
                continue;
            }
            let length = rng.random_range(6..=ROUNDS - 2);
            lengths.insert(length);
            let episode = clustered_episode(&mut rng, length);
            let normal = unstructured(seed, ROUNDS - length);
            let at = rng.random_range(1..=normal.len());
            let input = generate(seed);
            assert!(normal.len() >= 2);
            if input.clustered.is_none() {
                assert_eq!(input.rounds, unstructured(seed, ROUNDS));
                continue;
            }
            assert_eq!(input.clustered, Some(at..at + length));
            assert_eq!(input.rounds[..at], normal[..at]);
            assert_eq!(input.rounds[at..at + length], episode);
            assert_eq!(input.rounds[at + length..], normal[at..]);
            for round in episode {
                let generated = updates(&round);
                assert_eq!(generated.len(), UPDATES);
                assert!(generated.iter().all(|change| matches!(
                    change,
                    Change::Account { .. } | Change::DeleteAccount { .. }
                )));
                assert!(matches!(round.last(), Some(Action::UpdateBatch { .. })));
                assert_eq!(
                    round
                        .iter()
                        .filter(|action| matches!(action, Action::UpdateBatch { .. }))
                        .count(),
                    1
                );
                let hints = round
                    .iter()
                    .filter_map(|action| match action {
                        Action::Prewarm { key, .. } => Some(*key),
                        _ => None,
                    })
                    .collect::<BTreeSet<_>>();
                if hints.is_empty() {
                    cold = true;
                } else {
                    prewarmed = true;
                    assert_eq!(hints, generated.iter().map(|u| u.key()).collect());
                }
            }
        }
        assert_eq!(lengths, (6..=14).collect());
        assert!(prewarmed && cold);
    }

    fn updates(round: &[Action]) -> Vec<&Change> {
        round
            .iter()
            .flat_map(|action| match action {
                Action::UpdateBatch { updates } => updates.iter(),
                _ => [].iter(),
            })
            .collect()
    }

    #[test]
    fn lifecycle_updates_visit_active_before_repeating_and_recreate_empty() {
        let active = (1..=3).map(B256::with_last_byte).collect::<Vec<_>>();
        let mut deletion = false;
        let mut storage = false;
        for seed in 0..128 {
            for weight in 0..=4 {
                let mut known = Model::new();
                for _ in 0..4 {
                    let generated = generate_updates(
                        &mut StdRng::seed_from_u64(seed),
                        &mut known,
                        &active,
                        weight,
                    );
                    assert_eq!(
                        generated
                            .iter()
                            .take(active.len())
                            .map(|u| B256::from(u.key()))
                            .collect::<Vec<_>>(),
                        active
                    );
                    deletion |= generated.iter().any(|u| matches!(u, Change::DeleteAccount { .. }));
                    storage |= generated.iter().any(|u| matches!(u, Change::Storage { .. }));
                }
            }
        }
        assert!(deletion && storage);
        let key = active[0];
        let mut model = Model::new();
        update_model(
            &mut model,
            &Change::Account { key: key.0, nonce: 1, value: U256::ONE.to_be_bytes() },
        );
        update_model(
            &mut model,
            &Change::Storage { key: key.0, slot: [0x55; 32], value: U256::ONE.to_be_bytes() },
        );
        update_model(&mut model, &Change::DeleteAccount { key: key.0 });
        let generated = generate_updates(&mut StdRng::seed_from_u64(0), &mut model, &[key], 4);
        assert!(matches!(generated[0], Change::Account { nonce: 1, .. }));
        assert!(model
            .values()
            .all(|account| account.storage.is_empty() && account.touched.is_empty()));
    }

    #[test]
    fn touched_slots_survive_zero_writes_but_not_account_deletion() {
        let key = [0x11; 32];
        let slot = [0x22; 32];
        let mut state = Model::new();
        update_model(
            &mut state,
            &Change::Account { key, nonce: 1, value: U256::ONE.to_be_bytes() },
        );
        for value in [U256::ONE, U256::ZERO, U256::ZERO] {
            update_model(&mut state, &Change::Storage { key, slot, value: value.to_be_bytes() });
        }
        assert!(state[&B256::from(key)].storage.is_empty());
        assert_eq!(state[&B256::from(key)].touched, BTreeSet::from([B256::from(slot)]));
        update_model(&mut state, &Change::DeleteAccount { key });
        update_model(
            &mut state,
            &Change::Account { key, nonce: 1, value: U256::ONE.to_be_bytes() },
        );
        assert!(state[&B256::from(key)].touched.is_empty());
    }
}
