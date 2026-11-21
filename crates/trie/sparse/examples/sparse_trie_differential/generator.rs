//! Stateful storage-leaf input generation with normal and clustered working-set episodes.
//!
//! Account identities become prehashed leaves: account-only episodes rewrite nonzero leaf values,
//! while storage-heavy episodes can delete or rewrite those same leaves. There is no second-level
//! slot keyspace, account metadata, prefix-preimage dataset, or background-worker scheduling here.
//! Round boundaries replace commits; the driver retains its independent mandatory trait checks.

use super::{Action, Change, Input, INPUT_VERSION};
use alloy_primitives::{B256, U256};
use rand::{rngs::StdRng, seq::SliceRandom, Rng, SeedableRng};
use std::collections::BTreeSet;

const ROUNDS: usize = 16;
const UPDATES: usize = 4;

pub(super) fn generate(seed: u64) -> Input {
    let mut rng = StdRng::seed_from_u64(seed ^ 0x636c_7573_7465_7200);
    // Retain plain histories and at least one normal episode in every mixed history.
    let rounds = if rng.random_ratio(1, 3) {
        unstructured(seed, ROUNDS)
    } else {
        let length = rng.random_range(6..=ROUNDS - 2);
        let episode = clustered_episode(&mut rng, length);
        let mut rounds = unstructured(seed, ROUNDS - length);
        let at = rng.random_range(1..=rounds.len());
        rounds.splice(at..at, episode);
        rounds
    };
    Input { version: INPUT_VERSION, fixture_seed: seed, rounds }
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
    let mut known = population[..introduced].iter().copied().collect::<BTreeSet<_>>();
    let initialized = population[..introduced]
        .iter()
        .map(|key| Change { key: key.0, value: U256::ONE.to_be_bytes() })
        .collect();
    let mut rounds_out = vec![vec![Action::UpdateBatch { updates: initialized }]];
    let mut hot = Vec::new();
    let mut remaining = 0;
    let mut rewrite_weight = 0;
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
            rewrite_weight = population_rng.random_range(0..=4);
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
        let generated = generate_updates(&mut rng, &mut known, &active, rewrite_weight);
        let mut targets = generated.iter().map(|update| B256::from(update.key)).collect::<Vec<_>>();
        targets.shuffle(&mut rng);
        targets.truncate(rng.random_range(1..targets.len()));
        for _ in 0..rng.random_range(1..=8) {
            // These hints deliberately miss the population, including future introductions.
            let mut key = B256::from(rng.random::<[u8; 32]>());
            while population.contains(&key) || targets.contains(&key) {
                key = B256::from(rng.random::<[u8; 32]>());
            }
            targets.push(key);
        }
        targets.shuffle(&mut rng);
        let mut targets = targets.into_iter().peekable();
        let mut updates = generated.into_iter().peekable();
        let mut actions = Vec::new();
        while targets.peek().is_some() || updates.peek().is_some() {
            if targets.peek().is_some() && (updates.peek().is_none() || rng.random_bool(0.5)) {
                let size = rng.random_range(1..=targets.len().min(16));
                actions
                    .extend(targets.by_ref().take(size).map(|key| Action::Reveal { key: key.0 }));
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
    let mut known = BTreeSet::new();
    targets
        .into_iter()
        .map(|active| {
            let generated = generate_updates(rng, &mut known, &active, 4);
            let mut actions = Vec::new();
            if rng.random_bool(0.5) {
                actions.extend(active.into_iter().map(|key| Action::Reveal { key: key.0 }));
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
    known: &mut BTreeSet<B256>,
    active: &[B256],
    rewrite_weight: u32,
) -> Vec<Change> {
    (0..UPDATES)
        .map(|index| {
            // Visit every active leaf before drawing repeats; cold identities stay untouched.
            let key = active
                [if index < active.len() { index } else { rng.random_range(0..active.len()) }];
            // Remember identities after zero writes, just like account/touched-slot history.
            let value = if known.insert(key) || rng.random_ratio(rewrite_weight, 4) {
                rng.random_range(1..=1_000_000u64)
            } else if rng.random_ratio(1, 3) {
                0
            } else {
                rng.random::<u64>()
            };
            Change { key: key.0, value: U256::from(value).to_be_bytes() }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn histories_are_deterministic_bounded_and_keep_initialization() {
        let mut plain = false;
        let mut mixed = false;
        for seed in (0..128).chain([u64::MAX]) {
            let input = generate(seed);
            assert_eq!(input, generate(seed));
            assert_eq!(input.version, INPUT_VERSION);
            assert_eq!(input.fixture_seed, seed);
            assert_eq!(input.rounds.len(), ROUNDS);
            let initialized = updates(&input.rounds[0]);
            assert!((1..=UPDATES).contains(&initialized.len()));
            assert!(initialized
                .iter()
                .all(|update| U256::from_be_bytes(update.value) == U256::ONE));
            for (number, round) in input.rounds.iter().enumerate() {
                assert!(!round.is_empty() && round.len() <= 32);
                assert!(round.iter().all(|action| match action {
                    Action::UpdateBatch { updates } =>
                        !updates.is_empty() && updates.len() <= UPDATES,
                    Action::Reveal { .. } => true,
                    _ => false,
                }));
                if number != 0 {
                    assert_eq!(updates(round).len(), UPDATES);
                }
            }
            let normal = unstructured(seed, ROUNDS);
            plain |= input.rounds == normal;
            mixed |= input.rounds != normal;
        }
        assert!(plain && mixed);
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
            let mut known = updates(&rounds[0]).iter().map(|u| u.key).collect::<BTreeSet<_>>();
            let mut previous = known.clone();
            for round in &rounds[1..] {
                let generated = updates(round);
                let active = generated.iter().map(|u| u.key).collect::<BTreeSet<_>>();
                assert_eq!(generated.len(), UPDATES);
                assert!(!active.is_empty() && active.len() <= UPDATES);
                sizes.insert(active.len());
                staged |= !active.is_subset(&known);
                persisted |= !active.is_disjoint(&previous);
                rotated |= !previous.is_subset(&active);
                deletion |= generated.iter().any(|u| u.value == [0; 32]);
                let hints = round
                    .iter()
                    .filter_map(|action| match action {
                        Action::Reveal { key } => Some(*key),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                assert!((2..=11).contains(&hints.len()));
                assert!(hints.iter().any(|key| active.contains(key)));
                assert!(hints.iter().any(|key| !active.contains(key)));
                reveal_before |= matches!(round.first(), Some(Action::Reveal { .. }));
                reveal_after |= matches!(round.last(), Some(Action::Reveal { .. }));
                known.extend(&active);
                previous = active;
            }
        }
        assert!(staged && persisted && rotated && deletion);
        assert!(reveal_before && reveal_after);
        assert_eq!(sizes, BTreeSet::from([1, 2, 3, 4]));
    }

    #[test]
    fn leaf_updates_visit_active_before_repeating_and_remember_touched() {
        let active = (1..=3).map(B256::with_last_byte).collect::<Vec<_>>();
        let mut deletion = false;
        let mut storage_value = false;
        for seed in 0..128 {
            for weight in 0..=4 {
                let mut known = active.iter().copied().collect::<BTreeSet<_>>();
                let mut expected_known = known.clone();
                let generated =
                    generate_updates(&mut StdRng::seed_from_u64(seed), &mut known, &active, weight);
                assert_eq!(
                    generated
                        .iter()
                        .take(active.len())
                        .map(|u| B256::from(u.key))
                        .collect::<Vec<_>>(),
                    active
                );
                for update in generated {
                    let key = B256::from(update.key);
                    let value = U256::from_be_bytes(update.value);
                    if weight == 4 || !expected_known.contains(&key) {
                        assert!((U256::ONE..=U256::from(1_000_000)).contains(&value));
                    }
                    deletion |= value.is_zero();
                    storage_value |= value > U256::from(1_000_000);
                    expected_known.insert(key);
                }
                assert_eq!(known, expected_known);
                let mut absent = BTreeSet::new();
                let created = generate_updates(
                    &mut StdRng::seed_from_u64(seed),
                    &mut absent,
                    &active,
                    weight,
                );
                assert!(created[..active.len()].iter().all(|u| u.value != [0; 32]));
            }
        }
        assert!(deletion && storage_value);
    }

    #[test]
    fn touched_leaves_allow_repeated_deletions() {
        let active = [B256::with_last_byte(1)];
        let mut repeated_deletion = false;
        for seed in 0..128 {
            let mut rng = StdRng::seed_from_u64(seed);
            let mut known = BTreeSet::new();
            let mut previous = None;
            for _ in 0..4 {
                for update in generate_updates(&mut rng, &mut known, &active, 0) {
                    if previous.is_none() {
                        assert_ne!(update.value, [0; 32], "first write creates the leaf");
                    }
                    repeated_deletion |= previous == Some([0; 32]) && update.value == [0; 32];
                    previous = Some(update.value);
                    assert_eq!(known, BTreeSet::from(active));
                }
            }
        }
        assert!(repeated_deletion, "zero writes must not forget touched identities");
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
            assert_eq!(input.rounds[..at], normal[..at]);
            assert_eq!(input.rounds[at..at + length], episode);
            assert_eq!(input.rounds[at + length..], normal[at..]);
            for round in episode {
                let generated = updates(&round);
                assert_eq!(generated.len(), UPDATES);
                assert!(generated
                    .iter()
                    .all(|u| (U256::ONE..=U256::from(1_000_000))
                        .contains(&U256::from_be_bytes(u.value))));
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
                        Action::Reveal { key } => Some(*key),
                        _ => None,
                    })
                    .collect::<BTreeSet<_>>();
                if hints.is_empty() {
                    cold = true;
                } else {
                    prewarmed = true;
                    assert_eq!(hints, generated.iter().map(|u| u.key).collect());
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
}
