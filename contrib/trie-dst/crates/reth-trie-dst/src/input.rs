//! Explicit trie-task operation stream. Only `Commit` ends a block.

use crate::model::{apply, generate, random_address, random_slot, Model, Update};
use alloy_primitives::{Address, B256, KECCAK256_EMPTY, U256};
use eyre::{ensure, Result};
use rand::{rngs::StdRng, seq::SliceRandom, Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Input {
    pub operations: Vec<Operation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StorageHint {
    pub address: Address,
    pub slot: U256,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Operation {
    UpdateBatch { updates: Vec<Update> },
    PrewarmBatch { accounts: Vec<Address>, storages: Vec<StorageHint> },
    Commit,
}

#[derive(Serialize)]
pub(crate) struct MutationSite {
    pub operator: &'static str,
    pub path: Vec<String>,
}

impl Input {
    pub(crate) fn generate(seed: u64, blocks: u64, updates: usize) -> Self {
        let mut state = Model::default();
        let mut operations = Vec::new();
        for number in 1..=blocks {
            let mut rng = StdRng::seed_from_u64(seed.wrapping_add(number));
            let generated = generate(&mut rng, &state, updates);
            apply(&mut state, number, &generated);
            let mut targets = generated
                .iter()
                .map(|update| match update {
                    Update::Account { address, .. } => (*address, None),
                    Update::Storage { address, slot, .. } => (*address, Some(*slot)),
                })
                .collect::<Vec<_>>();
            targets.shuffle(&mut rng);
            let count =
                if targets.len() > 1 { rng.random_range(1..targets.len()) } else { targets.len() };
            targets.truncate(count);
            for _ in 0..rng.random_range(1..=8) {
                let address = random_address(&mut rng);
                let slot = rng.random_bool(0.5).then(|| random_slot(&mut rng));
                targets.push((address, slot));
            }
            targets.shuffle(&mut rng);
            let mut targets = targets.into_iter().peekable();
            let mut updates = generated.into_iter().peekable();
            while targets.peek().is_some() || updates.peek().is_some() {
                if targets.peek().is_some() && (updates.peek().is_none() || rng.random_bool(0.5)) {
                    let size = rng.random_range(1..=targets.len().min(16));
                    let mut accounts = Vec::new();
                    let mut storages = Vec::new();
                    for (address, slot) in targets.by_ref().take(size) {
                        match slot {
                            Some(slot) => storages.push(StorageHint { address, slot }),
                            None => accounts.push(address),
                        }
                    }
                    operations.push(Operation::PrewarmBatch { accounts, storages });
                } else {
                    let size = rng.random_range(1..=updates.len().min(16));
                    operations.push(Operation::UpdateBatch {
                        updates: updates.by_ref().take(size).collect(),
                    });
                }
            }
            operations.push(Operation::Commit);
        }
        Self { operations }
    }

    pub(crate) fn blocks(&self) -> Vec<Vec<Operation>> {
        self.operations
            .split_inclusive(|op| matches!(op, Operation::Commit))
            .map(<[Operation]>::to_vec)
            .collect()
    }

    pub(crate) fn validate(&self) -> Result<()> {
        let mut accounts = BTreeSet::new();
        let mut blocks = 0;
        let mut block_updates = 0;
        for operation in &self.operations {
            match operation {
                Operation::UpdateBatch { updates } => {
                    ensure!(!updates.is_empty(), "empty update batch");
                    block_updates += updates.len();
                    ensure!(block_updates <= 4096, "block needs at most 4096 updates");
                    for update in updates {
                        match update {
                            Update::Account { address, account } => {
                                ensure!(!account.is_empty(), "empty account: {address}");
                                ensure!(
                                    account.bytecode_hash != Some(KECCAK256_EMPTY),
                                    "noncanonical empty code hash: {address}"
                                );
                                accounts.insert(*address);
                            }
                            Update::Storage { address, .. } => {
                                ensure!(
                                    accounts.contains(address),
                                    "storage before account creation: {address}"
                                );
                            }
                        }
                    }
                }
                Operation::PrewarmBatch { accounts, storages } => {
                    ensure!(!accounts.is_empty() || !storages.is_empty(), "empty prewarm batch");
                }
                Operation::Commit => {
                    ensure!(block_updates > 0, "commit without updates");
                    blocks += 1;
                    ensure!(blocks <= 1024, "input needs at most 1024 commits");
                    block_updates = 0;
                }
            }
        }
        ensure!(
            matches!(self.operations.last(), Some(Operation::Commit)),
            "input must end with a commit"
        );
        Ok(())
    }

    pub(crate) fn mutation_sites(&self, only: Option<&str>) -> Vec<MutationSite> {
        let mut sites = Vec::new();
        for (i, op) in self.operations.iter().enumerate() {
            let root = vec!["operations".into(), i.to_string()];
            match op {
                Operation::UpdateBatch { updates } => {
                    if only.is_none_or(|name| {
                        name.starts_with("update.") ||
                            name.starts_with("account.") ||
                            name.starts_with("storage.")
                    }) {
                        for (j, update) in updates.iter().enumerate() {
                            let mut path = root.clone();
                            path.extend(["updates".into(), j.to_string()]);
                            for operator in [
                                "update.duplicate",
                                "update.remove",
                                "update.crossover",
                                "update.move",
                            ] {
                                if only.is_none_or(|name| name == operator) {
                                    sites.push(MutationSite { operator, path: path.clone() });
                                }
                            }
                            let (variant, fields): (&str, &[&str]) = match update {
                                Update::Account { .. } => (
                                    "Account",
                                    &[
                                        "address",
                                        "account/balance",
                                        "account/nonce",
                                        "account/bytecode_hash",
                                    ],
                                ),
                                Update::Storage { .. } => {
                                    ("Storage", &["address", "slot", "value"])
                                }
                            };
                            for field in fields {
                                let operator = match (variant, *field) {
                                    ("Account", "address") => "account.address",
                                    ("Account", "account/balance") => "account.balance",
                                    ("Account", "account/nonce") => "account.nonce",
                                    ("Account", _) => "account.bytecode_hash",
                                    ("Storage", "address") => "storage.address",
                                    ("Storage", "slot") => "storage.slot",
                                    _ => "storage.value",
                                };
                                if only.is_none_or(|name| name == operator) {
                                    let mut field_path = path.clone();
                                    field_path.push(variant.into());
                                    field_path.extend(field.split('/').map(str::to_string));
                                    sites.push(MutationSite { operator, path: field_path });
                                }
                            }
                        }
                    }
                    for operator in ["account.insert", "storage.insert", "storage.delete"] {
                        if only.is_none_or(|name| name == operator) {
                            sites.push(MutationSite { operator, path: root.clone() });
                        }
                    }
                    if only.is_none_or(|name| name == "commit.insert") {
                        sites.push(MutationSite { operator: "commit.insert", path: root.clone() });
                    }
                }
                Operation::PrewarmBatch { accounts, storages } => {
                    for operator in ["prewarm.account.insert", "prewarm.storage.insert"] {
                        if only.is_none_or(|name| name == operator) {
                            sites.push(MutationSite { operator, path: root.clone() });
                        }
                    }
                    for (field, count, operators) in [
                        (
                            "accounts",
                            accounts.len(),
                            ["prewarm.account.remove", "prewarm.account.move"],
                        ),
                        (
                            "storages",
                            storages.len(),
                            ["prewarm.storage.remove", "prewarm.storage.move"],
                        ),
                    ] {
                        for j in 0..count {
                            let mut path = root.clone();
                            path.extend([field.into(), j.to_string()]);
                            for operator in operators {
                                if only.is_none_or(|name| name == operator) {
                                    sites.push(MutationSite { operator, path: path.clone() });
                                }
                            }
                        }
                    }
                    if only.is_none_or(|name| name == "prewarm.account") {
                        for j in 0..accounts.len() {
                            let mut path = root.clone();
                            path.extend(["accounts".into(), j.to_string()]);
                            sites.push(MutationSite { operator: "prewarm.account", path });
                        }
                    }
                    if only.is_none_or(|name| name.starts_with("prewarm.storage.")) {
                        for j in 0..storages.len() {
                            let mut path = root.clone();
                            path.extend(["storages".into(), j.to_string()]);
                            for (operator, field) in [
                                ("prewarm.storage.address", "address"),
                                ("prewarm.storage.slot", "slot"),
                            ] {
                                if only.is_none_or(|name| name == operator) {
                                    let mut field_path = path.clone();
                                    field_path.push(field.into());
                                    sites.push(MutationSite { operator, path: field_path });
                                }
                            }
                        }
                    }
                }
                Operation::Commit => {
                    if i + 1 < self.operations.len() &&
                        only.is_none_or(|name| name == "commit.remove")
                    {
                        sites.push(MutationSite { operator: "commit.remove", path: root });
                    }
                    continue;
                }
            }
            for operator in ["batch.duplicate", "batch.remove", "batch.crossover", "batch.move"] {
                if only.is_none_or(|name| name == operator) {
                    sites.push(MutationSite { operator, path: root.clone() });
                }
            }
        }
        sites
    }

    pub(crate) fn mutate_at(
        &mut self,
        operator: &str,
        path: &[String],
        seed: u64,
        donor: Option<(&Self, &[String])>,
    ) -> Result<()> {
        self.validate()?;
        let original = self.clone();
        let mut rng = StdRng::seed_from_u64(seed);
        ensure!(path.first().map(String::as_str) == Some("operations"), "invalid mutation path");
        let i: usize =
            path.get(1).ok_or_else(|| eyre::eyre!("missing operation index"))?.parse()?;
        match operator {
            "account.insert" | "storage.insert" | "storage.delete" => {
                ensure!(path.len() == 2, "insertion needs a batch path");
                let Some(Operation::UpdateBatch { updates }) = self.operations.get(i) else {
                    eyre::bail!("not an update batch")
                };
                let j = rng.random_range(0..=updates.len());
                let update = if operator == "account.insert" {
                    let mut generated = generate(&mut rng, &Model::default(), 1);
                    let mut update =
                        generated.pop().ok_or_else(|| eyre::eyre!("no generated account"))?;
                    if let Update::Account { address, .. } = &mut update {
                        while self.operations.iter().any(|op| matches!(op,
                            Operation::UpdateBatch { updates } if updates.iter().any(|update|
                                matches!(update, Update::Account { address: existing, .. } if existing == address))))
                        {
                            *address = random_address(&mut rng);
                        }
                    }
                    update
                } else {
                    let mut state = Model::default();
                    for (index, op) in self.operations.iter().enumerate().take(i + 1) {
                        if let Operation::UpdateBatch { updates } = op {
                            for update in
                                updates.iter().take(if index == i { j } else { updates.len() })
                            {
                                update.apply_to_model(&mut state);
                            }
                        }
                    }
                    if operator == "storage.delete" {
                        let live_slots = state
                            .iter()
                            .flat_map(|(address, account)| {
                                account.storage.keys().map(move |slot| (*address, *slot))
                            })
                            .collect::<Vec<_>>();
                        if live_slots.is_empty() {
                            return Ok(());
                        }
                        let (address, slot) = live_slots[rng.random_range(0..live_slots.len())];
                        // Preserve the earlier nonzero write so this actually deletes a live slot.
                        Update::Storage { address, slot, value: U256::ZERO }
                    } else {
                        if state.is_empty() {
                            return Ok(());
                        }
                        let Some((address, account)) =
                            state.iter().nth(rng.random_range(0..state.len()))
                        else {
                            eyre::bail!("missing established account")
                        };
                        let mut slot = random_slot(&mut rng);
                        while account.touched_slots.contains(&slot) {
                            slot = random_slot(&mut rng);
                        }
                        Update::Storage {
                            address: *address,
                            slot,
                            value: U256::from(rng.random_range(1..=u64::MAX)),
                        }
                    }
                };
                if let Some(Operation::UpdateBatch { updates }) = self.operations.get_mut(i) {
                    updates.insert(j, update);
                }
            }
            "prewarm.account.insert" | "prewarm.storage.insert" => {
                ensure!(path.len() == 2, "insertion needs a batch path");
                let Some(Operation::PrewarmBatch { accounts, storages }) =
                    self.operations.get_mut(i)
                else {
                    eyre::bail!("not a prewarm batch")
                };
                if operator == "prewarm.account.insert" {
                    let mut address = random_address(&mut rng);
                    while accounts.contains(&address) {
                        address = random_address(&mut rng);
                    }
                    accounts.insert(rng.random_range(0..=accounts.len()), address);
                } else {
                    let mut hint = StorageHint {
                        address: random_address(&mut rng),
                        slot: random_slot(&mut rng),
                    };
                    while storages.contains(&hint) {
                        hint = StorageHint {
                            address: random_address(&mut rng),
                            slot: random_slot(&mut rng),
                        };
                    }
                    storages.insert(rng.random_range(0..=storages.len()), hint);
                }
            }
            "update.move" => {
                ensure!(
                    path.len() == 4 && path.get(2).map(String::as_str) == Some("updates"),
                    "invalid update path"
                );
                let j: usize =
                    path.get(3).ok_or_else(|| eyre::eyre!("missing update index"))?.parse()?;
                let Some(Operation::UpdateBatch { updates }) = self.operations.get(i) else {
                    eyre::bail!("not an update batch")
                };
                ensure!(j < updates.len(), "update out of range");
                if let Some((target, position)) = self.move_target(i, j, "updates", &mut rng) {
                    let Operation::UpdateBatch { updates } = &mut self.operations[i] else {
                        unreachable!()
                    };
                    let update = updates.remove(j);
                    let Operation::UpdateBatch { updates } = &mut self.operations[target] else {
                        unreachable!()
                    };
                    updates.insert(position, update);
                    self.remove_empty_batch(i);
                }
            }
            "prewarm.account.remove" |
            "prewarm.storage.remove" |
            "prewarm.account.move" |
            "prewarm.storage.move" => {
                let field =
                    if operator.starts_with("prewarm.account.") { "accounts" } else { "storages" };
                ensure!(
                    path.len() == 4 && path.get(2).map(String::as_str) == Some(field),
                    "invalid hint path"
                );
                let j: usize =
                    path.get(3).ok_or_else(|| eyre::eyre!("missing hint index"))?.parse()?;
                let Some(Operation::PrewarmBatch { accounts, storages }) = self.operations.get(i)
                else {
                    eyre::bail!("not a prewarm batch")
                };
                ensure!(
                    j < if field == "accounts" { accounts.len() } else { storages.len() },
                    "hint out of range"
                );
                let target = if operator.ends_with(".move") {
                    let Some(target) = self.move_target(i, j, field, &mut rng) else {
                        return Ok(());
                    };
                    Some(target)
                } else {
                    None
                };
                let Operation::PrewarmBatch { accounts, storages } = &mut self.operations[i] else {
                    unreachable!()
                };
                if field == "accounts" {
                    let hint = accounts.remove(j);
                    if let Some((target, position)) = target {
                        let Operation::PrewarmBatch { accounts, .. } = &mut self.operations[target]
                        else {
                            unreachable!()
                        };
                        accounts.insert(position, hint);
                    }
                } else {
                    let hint = storages.remove(j);
                    if let Some((target, position)) = target {
                        let Operation::PrewarmBatch { storages, .. } = &mut self.operations[target]
                        else {
                            unreachable!()
                        };
                        storages.insert(position, hint);
                    }
                }
                self.remove_empty_batch(i);
            }
            "batch.move" => {
                ensure!(
                    path.len() == 2 &&
                        !matches!(self.operations.get(i), Some(Operation::Commit) | None),
                    "batch move needs a batch path"
                );
                let positions = (0..self.operations.len() - 1)
                    .filter(|position| *position != i)
                    .collect::<Vec<_>>();
                if !positions.is_empty() {
                    let target = positions[rng.random_range(0..positions.len())];
                    let batch = self.operations.remove(i);
                    self.operations.insert(target, batch);
                }
            }
            "commit.insert" => {
                ensure!(
                    matches!(self.operations.get(i), Some(Operation::UpdateBatch { .. })),
                    "commit insertion needs an update batch"
                );
                self.operations.insert(i + 1, Operation::Commit);
            }
            "commit.remove" => {
                ensure!(
                    i + 1 < self.operations.len() &&
                        matches!(self.operations.get(i), Some(Operation::Commit)),
                    "cannot remove final commit"
                );
                self.operations.remove(i);
            }
            "batch.duplicate" => {
                let op = self
                    .operations
                    .get(i)
                    .ok_or_else(|| eyre::eyre!("operation out of range"))?
                    .clone();
                ensure!(!matches!(op, Operation::Commit), "cannot duplicate commit");
                self.operations.insert(i, op);
            }
            "batch.remove" => {
                ensure!(
                    !matches!(self.operations.get(i), Some(Operation::Commit) | None),
                    "cannot remove commit"
                );
                self.operations.remove(i);
            }
            "batch.crossover" => {
                let (other, other_path) = donor.ok_or_else(|| eyre::eyre!("missing donor"))?;
                ensure!(
                    other_path.first().map(String::as_str) == Some("operations"),
                    "invalid donor path"
                );
                let j: usize =
                    other_path.get(1).ok_or_else(|| eyre::eyre!("missing donor index"))?.parse()?;
                let replacement = other
                    .operations
                    .get(j)
                    .ok_or_else(|| eyre::eyre!("donor out of range"))?
                    .clone();
                ensure!(!matches!(replacement, Operation::Commit), "cannot cross commit");
                *self
                    .operations
                    .get_mut(i)
                    .ok_or_else(|| eyre::eyre!("operation out of range"))? = replacement;
            }
            "update.duplicate" | "update.remove" | "update.crossover" => {
                ensure!(path.get(2).map(String::as_str) == Some("updates"), "invalid update path");
                let j: usize =
                    path.get(3).ok_or_else(|| eyre::eyre!("missing update index"))?.parse()?;
                let replacement = if operator == "update.crossover" {
                    let (other, other_path) = donor.ok_or_else(|| eyre::eyre!("missing donor"))?;
                    ensure!(
                        other_path.first().map(String::as_str) == Some("operations") &&
                            other_path.get(2).map(String::as_str) == Some("updates"),
                        "invalid donor update path"
                    );
                    let oi: usize = other_path
                        .get(1)
                        .ok_or_else(|| eyre::eyre!("missing donor operation"))?
                        .parse()?;
                    let oj: usize = other_path
                        .get(3)
                        .ok_or_else(|| eyre::eyre!("missing donor update"))?
                        .parse()?;
                    let Some(Operation::UpdateBatch { updates }) = other.operations.get(oi) else {
                        eyre::bail!("donor is not an update batch")
                    };
                    Some(
                        updates
                            .get(oj)
                            .ok_or_else(|| eyre::eyre!("donor update out of range"))?
                            .clone(),
                    )
                } else {
                    None
                };
                let Some(Operation::UpdateBatch { updates }) = self.operations.get_mut(i) else {
                    eyre::bail!("not an update batch")
                };
                ensure!(j < updates.len(), "update out of range");
                match operator {
                    "update.duplicate" => updates.insert(j, updates[j].clone()),
                    "update.remove" => {
                        updates.remove(j);
                    }
                    _ => updates[j] = replacement.expect("crossover donor"),
                }
            }
            "account.address" |
            "account.balance" |
            "account.nonce" |
            "account.bytecode_hash" |
            "storage.address" |
            "storage.slot" |
            "storage.value" => {
                ensure!(path.get(2).map(String::as_str) == Some("updates"), "invalid update path");
                let j: usize =
                    path.get(3).ok_or_else(|| eyre::eyre!("missing update index"))?.parse()?;
                let available_accounts = if operator == "storage.address" {
                    self.operations
                        .iter()
                        .take(i + 1)
                        .enumerate()
                        .filter_map(|(index, op)| match op {
                            Operation::UpdateBatch { updates } => {
                                Some(updates.iter().take(if index == i {
                                    j
                                } else {
                                    updates.len()
                                }))
                            }
                            _ => None,
                        })
                        .flatten()
                        .filter_map(|update| match update {
                            Update::Account { address, .. } => Some(*address),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                } else {
                    Vec::new()
                };
                let mut renamed = None;
                let Some(Operation::UpdateBatch { updates }) = self.operations.get_mut(i) else {
                    eyre::bail!("not an update batch")
                };
                match updates.get_mut(j) {
                    Some(Update::Account { address, account })
                        if operator.starts_with("account.") =>
                    {
                        match operator {
                            "account.address" => {
                                let old = *address;
                                *address = random_address(&mut rng);
                                renamed = Some((old, *address));
                            }
                            "account.balance" => account.balance = U256::from(rng.random::<u64>()),
                            "account.nonce" => account.nonce = rng.random(),
                            _ => account.bytecode_hash = Some(B256::from(rng.random::<[u8; 32]>())),
                        }
                    }
                    Some(Update::Storage { address, slot, value })
                        if operator.starts_with("storage.") =>
                    {
                        match operator {
                            "storage.address" => {
                                if !available_accounts.is_empty() {
                                    *address = available_accounts
                                        [rng.random_range(0..available_accounts.len())];
                                }
                            }
                            "storage.slot" => *slot = random_slot(&mut rng),
                            _ => *value = U256::from(rng.random::<u64>()),
                        }
                    }
                    _ => eyre::bail!("update path/type mismatch"),
                }
                if let Some((old, new)) = renamed {
                    // Storage after this account update belongs to its new address until
                    // recreation.
                    'operations: for (index, op) in self.operations.iter_mut().enumerate().skip(i) {
                        if let Operation::UpdateBatch { updates } = op {
                            for (update_index, update) in updates.iter_mut().enumerate() {
                                if index == i && update_index <= j {
                                    continue;
                                }
                                match update {
                                    Update::Account { address, .. } if *address == old => {
                                        break 'operations;
                                    }
                                    Update::Storage { address, .. } if *address == old => {
                                        *address = new;
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
            "prewarm.account" | "prewarm.storage.address" | "prewarm.storage.slot" => {
                let j: usize =
                    path.get(3).ok_or_else(|| eyre::eyre!("missing hint index"))?.parse()?;
                let Some(Operation::PrewarmBatch { accounts, storages }) =
                    self.operations.get_mut(i)
                else {
                    eyre::bail!("not a prewarm batch")
                };
                if operator == "prewarm.account" {
                    ensure!(
                        path.get(2).map(String::as_str) == Some("accounts"),
                        "invalid account hint path"
                    );
                    *accounts
                        .get_mut(j)
                        .ok_or_else(|| eyre::eyre!("account hint out of range"))? =
                        random_address(&mut rng);
                } else {
                    ensure!(
                        path.get(2).map(String::as_str) == Some("storages"),
                        "invalid storage hint path"
                    );
                    let hint = storages
                        .get_mut(j)
                        .ok_or_else(|| eyre::eyre!("storage hint out of range"))?;
                    if operator == "prewarm.storage.address" {
                        hint.address = random_address(&mut rng);
                    } else {
                        hint.slot = random_slot(&mut rng);
                    }
                }
            }
            _ => eyre::bail!("unknown mutation operator: {operator}"),
        }
        if self.validate().is_err() {
            *self = original;
        }
        Ok(())
    }

    pub(crate) fn mutate(&mut self, seed: u64) -> Result<()> {
        let sites = self
            .mutation_sites(None)
            .into_iter()
            .filter(|site| !site.operator.ends_with(".crossover"))
            .collect::<Vec<_>>();
        let mut rng = StdRng::seed_from_u64(seed);
        let site = &sites[rng.random_range(0..sites.len())];
        self.mutate_at(site.operator, &site.path, rng.random(), None)
    }

    fn move_target(
        &self,
        source: usize,
        item: usize,
        field: &str,
        rng: &mut StdRng,
    ) -> Option<(usize, usize)> {
        let mut targets = Vec::new();
        for (index, op) in self.operations.iter().enumerate() {
            let len = match (field, op) {
                ("updates", Operation::UpdateBatch { updates }) => updates.len(),
                ("accounts", Operation::PrewarmBatch { accounts, .. }) => accounts.len(),
                ("storages", Operation::PrewarmBatch { storages, .. }) => storages.len(),
                _ => continue,
            };
            // Positions refer to the destination after removing the source item.
            for position in 0..=len - usize::from(index == source) {
                if index != source || position != item {
                    targets.push((index, position));
                }
            }
        }
        (!targets.is_empty()).then(|| targets[rng.random_range(0..targets.len())])
    }

    fn remove_empty_batch(&mut self, index: usize) {
        let empty = match &self.operations[index] {
            Operation::UpdateBatch { updates } => updates.is_empty(),
            Operation::PrewarmBatch { accounts, storages } => {
                accounts.is_empty() && storages.is_empty()
            }
            Operation::Commit => false,
        };
        if empty {
            self.operations.remove(index);
        }
    }
}
