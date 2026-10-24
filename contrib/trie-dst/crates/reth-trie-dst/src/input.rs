//! Explicit trie-task operation stream. Only `Commit` ends a block.

use crate::model::{apply, generate, random_address, random_slot, Model, Update};
use alloy_primitives::{Address, B256, KECCAK256_EMPTY, U256};
use eyre::{ensure, Result};
use rand::{rngs::StdRng, seq::SliceRandom, Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub operations: Vec<Operation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageHint {
    pub address: Address,
    pub slot: U256,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    UpdateBatch { updates: Vec<Update> },
    PrewarmBatch { accounts: Vec<Address>, storages: Vec<StorageHint> },
    Commit,
}

#[derive(Serialize)]
pub struct MutationSite {
    pub operator: &'static str,
    pub path: Vec<String>,
}

impl Input {
    pub fn generate(seed: u64, blocks: u64, updates: usize) -> Self {
        let mut state = Model::default();
        let mut operations = Vec::new();
        for number in 1..=blocks {
            let mut rng = StdRng::seed_from_u64(seed.wrapping_add(number));
            let generated = generate(&mut rng, &state, updates);
            apply(&mut state, number, &generated, [generated.len()]);
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

    pub fn blocks(&self) -> Vec<Vec<Operation>> {
        self.operations
            .split_inclusive(|op| matches!(op, Operation::Commit))
            .map(<[Operation]>::to_vec)
            .collect()
    }

    pub fn validate(&self) -> Result<()> {
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
        ensure!(blocks > 0 && block_updates == 0, "input must end with a commit");
        Ok(())
    }

    pub fn mutation_sites(&self, only: Option<&str>) -> Vec<MutationSite> {
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
                            for operator in
                                ["update.duplicate", "update.remove", "update.crossover"]
                            {
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
                    if only.is_none_or(|name| name == "commit.insert") {
                        sites.push(MutationSite { operator: "commit.insert", path: root.clone() });
                    }
                }
                Operation::PrewarmBatch { accounts, storages } => {
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
            for operator in ["batch.duplicate", "batch.remove", "batch.crossover"] {
                if only.is_none_or(|name| name == operator) {
                    sites.push(MutationSite { operator, path: root.clone() });
                }
            }
        }
        sites
    }

    pub fn mutate_at(
        &mut self,
        operator: &str,
        path: &[String],
        seed: u64,
        donor: Option<(&Input, &[String])>,
    ) -> Result<()> {
        self.validate()?;
        let original = self.clone();
        let mut rng = StdRng::seed_from_u64(seed);
        ensure!(path.first().map(String::as_str) == Some("operations"), "invalid mutation path");
        let i: usize =
            path.get(1).ok_or_else(|| eyre::eyre!("missing operation index"))?.parse()?;
        match operator {
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

    pub fn mutate(&mut self, seed: u64) -> Result<()> {
        let sites = self
            .mutation_sites(None)
            .into_iter()
            .filter(|site| !site.operator.ends_with(".crossover"))
            .collect::<Vec<_>>();
        let mut rng = StdRng::seed_from_u64(seed);
        let site = &sites[rng.random_range(0..sites.len())];
        self.mutate_at(site.operator, &site.path, rng.random(), None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_primitives_traits::Account;

    #[test]
    fn rejects_empty_accounts_but_accepts_hash_bearing_accounts() {
        let address = Address::from([1; 20]);
        let mut input = Input {
            operations: vec![
                Operation::UpdateBatch {
                    updates: vec![Update::Account { address, account: Account::default() }],
                },
                Operation::Commit,
            ],
        };
        assert!(input.validate().is_err());
        let Operation::UpdateBatch { updates } = &mut input.operations[0] else { unreachable!() };
        let Update::Account { account, .. } = &mut updates[0] else { unreachable!() };
        account.nonce = 1;
        assert!(input.validate().is_ok());
    }
}
