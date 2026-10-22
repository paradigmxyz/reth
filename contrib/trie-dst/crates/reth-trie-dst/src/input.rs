//! Explicit corpus inputs and deterministic, structure-preserving mutations.

use crate::model::{apply, generate, Model, Operation};
use alloy_primitives::{Address, U256};
use eyre::{ensure, Result};
use rand::{rngs::StdRng, Rng, SeedableRng};
use reth_primitives_traits::Account;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub blocks: Vec<BlockInput>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockInput {
    pub operations: Vec<Operation>,
    /// Controls batching and prewarming hints independently of operation generation.
    pub stream_seed: u64,
}

impl Input {
    pub fn generate(seed: u64, blocks: u64, updates: usize) -> Self {
        let mut state = Model::default();
        let blocks = (1..=blocks)
            .map(|number| {
                let mut rng = StdRng::seed_from_u64(seed.wrapping_add(number));
                let operations = generate(&mut rng, &state, updates);
                apply(&mut state, number, &operations);
                BlockInput { operations, stream_seed: seed.wrapping_add(number).rotate_left(32) }
            })
            .collect();
        Self { blocks }
    }

    pub fn validate(&self) -> Result<()> {
        ensure!((1..=1024).contains(&self.blocks.len()), "input needs 1..=1024 blocks");
        let mut accounts = BTreeSet::new();
        for block in &self.blocks {
            ensure!(
                (1..=4096).contains(&block.operations.len()),
                "block needs 1..=4096 operations"
            );
            for op in &block.operations {
                match op {
                    Operation::Account { address, account } => {
                        ensure!(!account.is_empty(), "empty account: {address}");
                        accounts.insert(*address);
                    }
                    Operation::Storage { address, .. } => {
                        ensure!(
                            accounts.contains(address),
                            "storage before account creation: {address}"
                        );
                    }
                }
            }
        }
        Ok(())
    }

    pub fn mutate(&mut self, seed: u64) -> Result<()> {
        self.validate()?;
        let mut rng = StdRng::seed_from_u64(seed);
        // Stack a few domain edits; do not mutate the serialized JSON bytes.
        for _ in 0..rng.random_range(1..=4) {
            let b = rng.random_range(0..self.blocks.len());
            match rng.random_range(0..7) {
                0 => self.blocks[b].stream_seed = rng.random(),
                1 if self.blocks.len() > 1 => {
                    self.blocks.remove(b);
                }
                2 if self.blocks.len() < 1024 => {
                    self.blocks.insert(b, self.blocks[b].clone());
                }
                kind => {
                    let ops = &mut self.blocks[b].operations;
                    let i = rng.random_range(0..ops.len());
                    match kind {
                        3 if ops.len() > 1 => {
                            ops.remove(i);
                        }
                        4 if ops.len() < 4095 => ops.insert(i, ops[i].clone()),
                        5 => {
                            let j = rng.random_range(0..ops.len());
                            ops.swap(i, j);
                        }
                        _ => match &mut ops[i] {
                            Operation::Account { account, .. } => {
                                account.balance = U256::from(rng.random::<u64>());
                                account.nonce = rng.random();
                            }
                            Operation::Storage { slot, value, .. } => {
                                *slot = U256::from(rng.random_range(0..64u64));
                                *value = if rng.random_bool(0.5) {
                                    U256::ZERO
                                } else {
                                    U256::from(rng.random::<u64>())
                                };
                            }
                        },
                    }
                }
            }
        }
        // Sequence edits can remove an account's producer. Materialize the prerequisite,
        // rather than panicking in the model or silently discarding a storage operation.
        let mut accounts = BTreeSet::<Address>::new();
        for block in &mut self.blocks {
            let mut repaired = Vec::new();
            for op in std::mem::take(&mut block.operations) {
                let address = match &op {
                    Operation::Account { address, .. } | Operation::Storage { address, .. } => {
                        *address
                    }
                };
                let prerequisite =
                    !accounts.contains(&address) && matches!(op, Operation::Storage { .. });
                if repaired.len() + 1 + usize::from(prerequisite) > 4096 {
                    break;
                }
                accounts.insert(address);
                if prerequisite {
                    repaired.push(Operation::Account {
                        address,
                        account: Account { nonce: 1, ..Default::default() },
                    });
                }
                repaired.push(op);
            }
            block.operations = repaired;
        }
        self.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_accounts_but_accepts_hash_bearing_accounts() {
        let address = Address::from([1; 20]);
        let mut input = Input {
            blocks: vec![BlockInput {
                operations: vec![Operation::Account { address, account: Account::default() }],
                stream_seed: 0,
            }],
        };
        assert_eq!(input.validate().unwrap_err().to_string(), format!("empty account: {address}"));
        let Operation::Account { account, .. } = &mut input.blocks[0].operations[0] else {
            unreachable!()
        };
        account.bytecode_hash = Some(alloy_primitives::B256::from([2; 32]));
        input.validate().unwrap();
    }
}
