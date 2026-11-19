//! Bounded structured-input planning. All edits are local, deterministic and in-process.

use crate::{
    core::{Action, Change, Input},
    mix, Error,
};
use serde::Serialize;
use serde_json::{json, Value};

// Two maximally escaped inputs plus paths and metadata still fit the 65536-byte ABI request.
const MAX_INPUT_BYTES: usize = 12_288;
const OPERATORS: [&str; 13] = [
    "action.insert",
    "action.remove",
    "action.duplicate",
    "action.crossover",
    "action.swap",
    "update.insert",
    "update.remove",
    "update.duplicate",
    "update.crossover",
    "key.update",
    "key.prefix",
    "value.update",
    "prune.update",
];

#[derive(Serialize)]
pub(crate) struct Site {
    operator: String,
    pub(crate) path: Vec<String>,
}

pub(crate) fn parse(text: &str) -> Result<Input, Error> {
    if text.len() > MAX_INPUT_BYTES {
        return Err(Error::Invalid("input exceeds 12288 bytes"));
    }
    let input: Input = serde_json::from_str(text)?;
    input.validate().map_err(Error::Invalid)?;
    Ok(input)
}

pub(crate) fn text(input: &Input) -> Result<String, Error> {
    input.validate().map_err(Error::Invalid)?;
    let text = serde_json::to_string(input)?;
    if text.len() > MAX_INPUT_BYTES {
        return Err(Error::Invalid("input exceeds 12288 bytes"));
    }
    Ok(text)
}

pub(crate) fn generated(seed: u64) -> Result<String, Error> {
    text(&Input::generate(seed))
}

pub(crate) fn operators() -> Value {
    // Omit static selectors: Rift bypasses mutation-sites when a selector is present, but
    // valid sites here depend on capacities, adjacency and mixed reveal/update key depths.
    Value::Array(
        OPERATORS
            .iter()
            .map(|name| {
                json!({
                    "name": name,
                    "donor": matches!(*name, "action.crossover" | "update.crossover" | "key.prefix")
                })
            })
            .collect(),
    )
}

pub(crate) fn sites(input: &Input, operator: &str) -> Result<Vec<Site>, Error> {
    input.validate().map_err(Error::Invalid)?;
    if !OPERATORS.contains(&operator) {
        return Err(Error::Invalid("unknown mutation operator"));
    }
    let mut paths = Vec::new();
    for (round, actions) in input.rounds.iter().enumerate() {
        let root = vec!["rounds".into(), round.to_string()];
        if operator == "action.insert" && actions.len() < 8 {
            paths.push(root.clone());
        }
        for (index, action) in actions.iter().enumerate() {
            let mut path = root.clone();
            path.push(index.to_string());
            match operator {
                "action.remove" | "action.crossover" => paths.push(path.clone()),
                "action.duplicate" if actions.len() < 8 => paths.push(path.clone()),
                "action.swap" if index + 1 < actions.len() => paths.push(path.clone()),
                _ => {}
            }
            match action {
                Action::UpdateBatch { updates } => {
                    if operator == "update.insert" && updates.len() < 4 {
                        paths.push(path.clone());
                    }
                    for index in 0..updates.len() {
                        let mut path = path.clone();
                        path.extend(["updates".into(), index.to_string()]);
                        match operator {
                            "update.remove" => paths.push(path),
                            "update.duplicate" if updates.len() < 4 => paths.push(path),
                            "update.crossover" => paths.push(path),
                            "key.update" | "key.prefix" => {
                                path.push("key".into());
                                paths.push(path);
                            }
                            "value.update" => {
                                path.push("value".into());
                                paths.push(path);
                            }
                            _ => {}
                        }
                    }
                }
                Action::Reveal { .. } if matches!(operator, "key.update" | "key.prefix") => {
                    path.push("key".into());
                    paths.push(path);
                }
                Action::Prune { .. } if operator == "prune.update" => {
                    path.push("cutoff".into());
                    paths.push(path);
                }
                _ => {}
            }
        }
    }
    Ok(paths.into_iter().map(|path| Site { operator: operator.into(), path }).collect())
}

fn checked_site(input: &Input, operator: &str, path: &[String]) -> Result<(), Error> {
    if sites(input, operator)?.iter().any(|site| site.path == path) {
        Ok(())
    } else {
        Err(Error::Invalid("invalid mutation path"))
    }
}

fn index(path: &[String], position: usize) -> Result<usize, Error> {
    path.get(position)
        .and_then(|part| part.parse().ok())
        .ok_or(Error::Invalid("invalid mutation path"))
}

fn action_mut<'a>(input: &'a mut Input, path: &[String]) -> Result<&'a mut Action, Error> {
    input
        .rounds
        .get_mut(index(path, 1)?)
        .and_then(|actions| actions.get_mut(index(path, 2).ok()?))
        .ok_or(Error::Invalid("invalid mutation path"))
}

fn updates_mut<'a>(input: &'a mut Input, path: &[String]) -> Result<&'a mut Vec<Change>, Error> {
    match action_mut(input, path)? {
        Action::UpdateBatch { updates } => Ok(updates),
        _ => Err(Error::Invalid("mutation needs an update batch")),
    }
}

fn key_mut<'a>(input: &'a mut Input, path: &[String]) -> Result<&'a mut [u8; 32], Error> {
    let update = if path.len() == 6 { Some(index(path, 4)?) } else { None };
    match (action_mut(input, path)?, update) {
        (Action::Reveal { key }, None) => Ok(key),
        (Action::UpdateBatch { updates }, Some(index)) => updates
            .get_mut(index)
            .map(|change| &mut change.key)
            .ok_or(Error::Invalid("invalid mutation path")),
        _ => Err(Error::Invalid("mutation needs a key")),
    }
}

pub(crate) fn mutate(
    mut input: Input,
    operator: &str,
    path: &[String],
    seed: u64,
    donor: Option<(Input, Vec<String>)>,
) -> Result<String, Error> {
    let original = text(&input)?;
    checked_site(&input, operator, path)?;
    let needs_donor = matches!(operator, "action.crossover" | "update.crossover" | "key.prefix");
    if donor.is_some() != needs_donor {
        return Err(Error::Invalid("mutation donor does not match operator"));
    }
    let mut donor = donor;
    if let Some((other, other_path)) = &donor {
        checked_site(other, operator, other_path)?;
    }
    let random = mix(seed);
    let round = index(path, 1)?;
    match operator {
        "action.insert" => {
            let at = (random % (input.rounds[round].len() + 1) as u64) as usize;
            let mut generated = Input::generate(seed);
            let action = match mix(random) % 6 {
                0 => generated.rounds[round].remove(0),
                1 => Action::Reveal { key: [random as u8; 32] },
                2 => Action::Prune { cutoff: mix(random) % 200 },
                3 => Action::Snapshot,
                4 => Action::Clone,
                _ => Action::TrackingReset,
            };
            input.rounds[round].insert(at, action);
        }
        "action.remove" | "action.duplicate" | "action.swap" => {
            let at = index(path, 2)?;
            let actions = &mut input.rounds[round];
            match operator {
                "action.remove" => {
                    actions.remove(at);
                }
                "action.duplicate" => actions.insert(at, actions[at].clone()),
                _ => actions.swap(at, at + 1),
            }
        }
        "action.crossover" => {
            let (other, other_path) = donor.as_mut().ok_or(Error::Invalid("missing donor"))?;
            let replacement = action_mut(other, other_path)?.clone();
            *action_mut(&mut input, path)? = replacement;
        }
        "update.insert" => {
            let mut generated = Input::generate(seed);
            let Action::UpdateBatch { mut updates } = generated.rounds[round].remove(0) else {
                unreachable!("generator starts each round with an update batch");
            };
            let target = updates_mut(&mut input, path)?;
            let at = (random % (target.len() + 1) as u64) as usize;
            target.insert(at, updates.remove(0));
        }
        "prune.update" => {
            let Action::Prune { cutoff } = action_mut(&mut input, path)? else {
                return Err(Error::Invalid("mutation needs a prune action"));
            };
            *cutoff = match random % 4 {
                0 if *cutoff != 0 => 0,
                1 if *cutoff != u64::MAX => u64::MAX,
                _ => cutoff.wrapping_add(1 + random % 200),
            };
        }
        "update.remove" | "update.duplicate" | "update.crossover" => {
            let at = index(path, 4)?;
            let updates = updates_mut(&mut input, path)?;
            match operator {
                "update.remove" => {
                    updates.remove(at);
                }
                "update.duplicate" => updates.insert(at, updates[at].clone()),
                _ => {
                    let (other, other_path) =
                        donor.as_mut().ok_or(Error::Invalid("missing donor"))?;
                    updates[at] = updates_mut(other, other_path)?[index(other_path, 4)?].clone();
                }
            }
        }
        "key.update" => {
            let key = key_mut(&mut input, path)?;
            key[(random % 32) as usize] ^= 1 << (mix(random) % 8);
        }
        "key.prefix" => {
            let (other, other_path) = donor.as_mut().ok_or(Error::Invalid("missing donor"))?;
            let source = *key_mut(other, other_path)?;
            let key = key_mut(&mut input, path)?;
            // Nibble-granular crossover retains the destination suffix and long shared prefixes.
            let nibbles = (random % 63 + 1) as usize;
            key[..nibbles / 2].copy_from_slice(&source[..nibbles / 2]);
            if !nibbles.is_multiple_of(2) {
                key[nibbles / 2] = (source[nibbles / 2] & 0xf0) | (key[nibbles / 2] & 0x0f);
            }
        }
        "value.update" => {
            let at = index(path, 4)?;
            let value = &mut updates_mut(&mut input, path)?[at].value;
            let old = *value;
            match random % 5 {
                0 => *value = [0; 32],
                1 => {
                    *value = [0; 32];
                    value[31] = 1;
                }
                2 => {
                    *value = [0; 32];
                    value[31] = 127;
                }
                3 => {
                    *value = [0; 32];
                    value[31] = 128;
                }
                _ => value[(random % 32) as usize] ^= 1 << (mix(random) % 8),
            }
            if *value == old {
                value[31] ^= 1;
            }
        }
        _ => return Err(Error::Invalid("unknown mutation operator")),
    }
    match text(&input) {
        // Rift treats planning errors as connection failures; exhausted size budgets skip an edit.
        Err(Error::Invalid("input exceeds 12288 bytes")) => Ok(original),
        result => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{core, Request, MAX_REQUEST_BYTES};

    fn path(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_owned()).collect()
    }

    #[test]
    fn generated_inputs_are_versioned_bounded_and_deterministic() {
        for seed in (0..72).chain([u64::MAX - 1, u64::MAX]) {
            let value = generated(seed).unwrap();
            assert_eq!(value, generated(seed).unwrap());
            let input = parse(&value).unwrap();
            assert_eq!(input.version, core::INPUT_VERSION);
            assert_eq!(input.fixture_seed, seed);
            assert_eq!(text(&input).unwrap(), value);
            // Test the actual escaped request, not just the unescaped input size.
            let request = json!({"method":"mutate-input","params":{
                "args":{}, "input":value, "donor":value, "seed":u64::MAX,
                "operator":"update.crossover", "path":["rounds","15","7","updates","3"],
                "donor_path":["rounds","15","7","updates","3"]
            }})
            .to_string();
            assert!(request.len() <= MAX_REQUEST_BYTES);
            serde_json::from_str::<Request>(&request).unwrap();
        }
    }

    #[test]
    fn every_operator_has_real_sites_and_deterministic_local_edits() {
        let input = Input::generate(0);
        let donor = Input::generate(1);
        let metadata = operators();
        for entry in metadata.as_array().unwrap() {
            assert!(entry.get("selector").is_none());
            let operator = entry["name"].as_str().unwrap();
            let locations = sites(&input, operator).unwrap();
            assert!(!locations.is_empty(), "{operator}");
            let location = &locations[0].path;
            let pointer = format!("/{}", location.join("/"));
            assert!(serde_json::to_value(&input).unwrap().pointer(&pointer).is_some());
            let other = entry["donor"].as_bool().unwrap().then(|| {
                let other_path = sites(&donor, operator).unwrap().remove(0).path;
                (donor.clone(), other_path)
            });
            let first = mutate(input.clone(), operator, location, 99, other.clone()).unwrap();
            assert_eq!(first, mutate(input.clone(), operator, location, 99, other).unwrap());
            let result = parse(&first).unwrap();
            assert_eq!(result.fixture_seed, input.fixture_seed);
            let round = index(location, 1).unwrap();
            assert_ne!(result.rounds[round], input.rounds[round], "{operator}");
            for index in 0..16 {
                if index != round {
                    assert_eq!(result.rounds[index], input.rounds[index], "{operator}");
                }
            }
        }
    }

    #[test]
    fn value_and_key_edits_preserve_other_fields() {
        let input = Input::generate(0);
        for operator in ["key.update", "value.update"] {
            let location = sites(&input, operator).unwrap().remove(0).path;
            let mut result =
                parse(&mutate(input.clone(), operator, &location, 99, None).unwrap()).unwrap();
            let result_key = *key_mut(&mut result, &location).unwrap();
            let mut expected = input.clone();
            if operator == "key.update" {
                *key_mut(&mut expected, &location).unwrap() = result_key;
            } else {
                let at = index(&location, 4).unwrap();
                updates_mut(&mut expected, &location).unwrap()[at].value =
                    updates_mut(&mut result, &location).unwrap()[at].value;
            }
            assert_eq!(result, expected);
        }
    }

    #[test]
    fn removals_shrink_to_empty_and_insertions_restore_sites() {
        let mut input = Input::generate(0);
        let batch = path(&["rounds", "0", "0", "updates", "0"]);
        input = parse(&mutate(input, "update.remove", &batch, 0, None).unwrap()).unwrap();
        assert_eq!(updates_mut(&mut input, &batch).unwrap().len(), 0);
        input =
            parse(&mutate(input, "update.insert", &path(&["rounds", "0", "0"]), 1, None).unwrap())
                .unwrap();
        assert_eq!(updates_mut(&mut input, &batch).unwrap().len(), 1);
        while let Some(location) = sites(&input, "action.remove").unwrap().first() {
            input =
                parse(&mutate(input.clone(), "action.remove", &location.path, 0, None).unwrap())
                    .unwrap();
        }
        assert!(input.rounds.iter().all(Vec::is_empty));
        assert_eq!(sites(&input, "action.insert").unwrap().len(), 16);
        let inserted =
            mutate(input.clone(), "action.insert", &path(&["rounds", "0"]), 0, None).unwrap();
        assert_eq!(parse(&inserted).unwrap().rounds[0].len(), 1);
        // Shrinking removes optional actions, never the mandatory oracle spine.
        assert_eq!(core::run_input(&input).unwrap(), core::run_input(&input).unwrap());
    }

    #[test]
    fn crossover_copies_exact_donor_action_or_update() {
        for operator in ["action.crossover", "update.crossover"] {
            let input = Input::generate(0);
            let mut donor = Input::generate(7);
            let target = sites(&input, operator).unwrap().remove(0).path;
            let source = sites(&donor, operator).unwrap().remove(1).path;
            let result = parse(
                &mutate(input.clone(), operator, &target, 0, Some((donor.clone(), source.clone())))
                    .unwrap(),
            )
            .unwrap();
            let mut expected = input;
            if operator == "action.crossover" {
                *action_mut(&mut expected, &target).unwrap() =
                    action_mut(&mut donor, &source).unwrap().clone();
            } else {
                updates_mut(&mut expected, &target).unwrap()[index(&target, 4).unwrap()] =
                    updates_mut(&mut donor, &source).unwrap()[index(&source, 4).unwrap()].clone();
            }
            assert_eq!(result, expected);
            let identical = mutate(
                result.clone(),
                operator,
                &target,
                0,
                Some((result.clone(), target.clone())),
            )
            .unwrap();
            assert_eq!(parse(&identical).unwrap(), result);
            result.validate().unwrap();
        }
    }

    #[test]
    fn prefix_crossover_preserves_destination_suffix() {
        let mut input = Input::generate(0);
        let mut donor = Input::generate(1);
        let location = sites(&input, "key.prefix").unwrap().remove(0).path;
        *key_mut(&mut input, &location).unwrap() = [0x12; 32];
        *key_mut(&mut donor, &location).unwrap() = [0xab; 32];
        let mut changed = parse(
            &mutate(input, "key.prefix", &location, 9, Some((donor, location.clone()))).unwrap(),
        )
        .unwrap();
        let key = key_mut(&mut changed, &location).unwrap();
        let nibbles = (mix(9) % 63 + 1) as usize;
        for nibble in 0..64 {
            let actual = if nibble % 2 == 0 { key[nibble / 2] >> 4 } else { key[nibble / 2] & 15 };
            let expected = if nibble < nibbles {
                if nibble % 2 == 0 {
                    10
                } else {
                    11
                }
            } else if nibble % 2 == 0 {
                1
            } else {
                2
            };
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn invalid_paths_donors_versions_and_bounds_are_rejected() {
        let input = Input::generate(0);
        for invalid in [
            path(&[]),
            path(&["rounds", "16", "0"]),
            path(&["rounds", "0", "999"]),
            path(&["rounds", "0", "0", "updates", "999", "key"]),
            path(&["rounds", "00", "0"]),
            path(&["seed"]),
            path(&["rounds", "0", "0", "key"]),
        ] {
            assert_eq!(
                mutate(input.clone(), "key.update", &invalid, 0, None).unwrap_err().to_string(),
                "invalid mutation path"
            );
        }
        let target = sites(&input, "action.crossover").unwrap().remove(0).path;
        assert_eq!(
            mutate(input.clone(), "action.crossover", &target, 0, None).unwrap_err().to_string(),
            "mutation donor does not match operator"
        );
        assert_eq!(
            mutate(
                input.clone(),
                "action.crossover",
                &target,
                0,
                Some((input.clone(), path(&["rounds", "99", "0"])))
            )
            .unwrap_err()
            .to_string(),
            "invalid mutation path"
        );
        assert_eq!(
            sites(&input, "unknown").err().unwrap().to_string(),
            "unknown mutation operator"
        );
        let mut invalid = input.clone();
        invalid.version = 1;
        assert_eq!(text(&invalid).unwrap_err().to_string(), "unsupported input version");
        invalid = input.clone();
        invalid.rounds.pop();
        assert_eq!(text(&invalid).unwrap_err().to_string(), "input requires sixteen rounds");
        invalid = input.clone();
        invalid.rounds[0] = vec![Action::Clone; 9];
        assert_eq!(text(&invalid).unwrap_err().to_string(), "round exceeds eight actions");
        invalid = input.clone();
        let updates = updates_mut(&mut invalid, &target).unwrap();
        *updates = vec![updates[0].clone(); 5];
        assert_eq!(text(&invalid).unwrap_err().to_string(), "batch exceeds four updates");
        assert_eq!(
            parse(&" ".repeat(MAX_INPUT_BYTES + 1)).unwrap_err().to_string(),
            "input exceeds 12288 bytes"
        );
        let mut padded = generated(0).unwrap();
        padded.extend(std::iter::repeat_n('\t', MAX_INPUT_BYTES - padded.len()));
        let request =
            json!({"method":"mutate-input","params":{"args":{},"input":padded,"donor":padded,
            "operator":"action.crossover","seed":u64::MAX,"path":target,"donor_path":target}})
            .to_string();
        assert!(request.len() < MAX_REQUEST_BYTES);
        assert_eq!(parse(&padded).unwrap(), input);
        let mut full = input;
        for actions in &mut full.rounds {
            *actions = vec![
                Action::UpdateBatch {
                    updates: vec![Change { key: [255; 32], value: [255; 32] }; 4]
                };
                8
            ];
        }
        assert_eq!(text(&full).unwrap_err().to_string(), "input exceeds 12288 bytes");
    }

    #[test]
    fn growth_at_byte_budget_is_a_safe_noop() {
        let mut input = Input::generate(0);
        loop {
            let mut grew = false;
            if let Some(site) = sites(&input, "update.duplicate").unwrap().into_iter().next() {
                let mut candidate = input.clone();
                let at = index(&site.path, 4).unwrap();
                let updates = updates_mut(&mut candidate, &site.path).unwrap();
                updates.insert(at, updates[at].clone());
                if text(&candidate).is_err() {
                    let original = text(&input).unwrap();
                    assert_eq!(
                        mutate(input, "update.duplicate", &site.path, 0, None).unwrap(),
                        original
                    );
                    return;
                }
                input = candidate;
                grew = true;
            }
            assert!(grew, "expected to exhaust bytes before update count limits");
        }
    }

    #[test]
    fn prune_cutoff_mutations_cover_zero_max_and_epoch_boundaries() {
        let mut input = Input::generate(0);
        input.rounds[0] = vec![Action::Prune { cutoff: 18 }];
        let target = sites(&input, "prune.update").unwrap().remove(0).path;
        let mut observed = Vec::new();
        for seed in 0..32 {
            let result =
                parse(&mutate(input.clone(), "prune.update", &target, seed, None).unwrap())
                    .unwrap();
            let Action::Prune { cutoff } = result.rounds[0][0] else { unreachable!() };
            assert_ne!(cutoff, 18);
            observed.push(cutoff);
        }
        assert!(observed.contains(&0));
        assert!(observed.contains(&u64::MAX));
        assert!(observed.iter().any(|cutoff| *cutoff > 18 && *cutoff < u64::MAX));
        for cutoff in [0, 17, 18, 19, u64::MAX] {
            input.rounds[0] = vec![Action::Prune { cutoff }];
            core::run_input(&input).unwrap();
        }
    }

    #[test]
    fn generated_and_mutated_tapes_execute_with_replay() {
        let input = Input::generate(0);
        for metadata in operators().as_array().unwrap() {
            let operator = metadata["name"].as_str().unwrap();
            let target = sites(&input, operator).unwrap().remove(0).path;
            let other = metadata["donor"].as_bool().unwrap().then(|| {
                let donor = Input::generate(1);
                let source = sites(&donor, operator).unwrap().remove(0).path;
                (donor, source)
            });
            let value = mutate(input.clone(), operator, &target, 99, other).unwrap();
            let parsed = parse(&value).unwrap();
            let first = core::run_input(&parsed).unwrap();
            assert_eq!(first, core::run_input(&parsed).unwrap(), "{operator}");
        }
    }
}
