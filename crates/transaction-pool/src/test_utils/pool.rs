//! Test helpers for mocking an entire pool.

#![allow(dead_code)]

use crate::{
    pool::{txpool::TxPool, AddedTransaction},
    test_utils::{MockOrdering, MockTransactionDistribution, MockTransactionFactory},
    TransactionOrdering,
};
use rand::Rng;
use reth_primitives::{Address, U256};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    ops::{Deref, DerefMut},
};

/// A wrapped `TxPool` with additional helpers for testing
pub(crate) struct MockPool<T: TransactionOrdering = MockOrdering> {
    // The wrapped pool.
    pool: TxPool<T>,
}

impl MockPool {
    /// The total size of all subpools
    fn total_subpool_size(&self) -> usize {
        self.pool.pending().len() + self.pool.base_fee().len() + self.pool.queued().len()
    }

    /// Checks that all pool invariants hold.
    fn enforce_invariants(&self) {
        assert_eq!(
            self.pool.len(),
            self.total_subpool_size(),
            "Tx in AllTransactions and sum(subpools) must match"
        );
    }
}

impl Default for MockPool {
    fn default() -> Self {
        Self { pool: TxPool::new(MockOrdering::default(), Default::default()) }
    }
}

impl<T: TransactionOrdering> Deref for MockPool<T> {
    type Target = TxPool<T>;

    fn deref(&self) -> &Self::Target {
        &self.pool
    }
}

impl<T: TransactionOrdering> DerefMut for MockPool<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.pool
    }
}

/// Simulates transaction execution.
pub(crate) struct MockTransactionSimulator<R: Rng> {
    /// The pending base fee
    base_fee: u128,
    /// Generator for transactions
    tx_generator: MockTransactionDistribution,
    /// represents the on chain balance of a sender.
    balances: HashMap<Address, U256>,
    /// represents the on chain nonce of a sender.
    nonces: HashMap<Address, u64>,
    /// A set of addresses to as senders.
    senders: Vec<Address>,
    /// What scenarios to execute.
    scenarios: Vec<ScenarioType>,
    /// All previous scenarios executed by a sender.
    executed: HashMap<Address, ExecutedScenarios>,
    /// "Validates" generated transactions.
    validator: MockTransactionFactory,
    /// The rng instance used to select senders and scenarios.
    rng: R,
}

impl<R: Rng> MockTransactionSimulator<R> {
    /// Returns a new mock instance
    pub(crate) fn new(mut rng: R, config: MockSimulatorConfig) -> Self {
        let senders = config.addresses(&mut rng);
        Self {
            base_fee: config.base_fee,
            balances: senders.iter().copied().map(|a| (a, rng.gen())).collect(),
            nonces: senders.iter().copied().map(|a| (a, 0)).collect(),
            senders,
            scenarios: config.scenarios,
            tx_generator: config.tx_generator,
            executed: Default::default(),
            validator: Default::default(),
            rng,
        }
    }

    /// Returns a random address from the senders set
    fn rng_address(&mut self) -> Address {
        let idx = self.rng.gen_range(0..self.senders.len());
        self.senders[idx]
    }

    /// Returns a random scenario from the scenario set
    fn rng_scenario(&mut self) -> ScenarioType {
        let idx = self.rng.gen_range(0..self.scenarios.len());
        self.scenarios[idx].clone()
    }

    /// Executes the next scenario and applies it to the pool
    pub(crate) fn next(&mut self, pool: &mut MockPool) {
        let sender = self.rng_address();
        let scenario = self.rng_scenario();
        let on_chain_nonce = self.nonces[&sender];
        let on_chain_balance = self.balances[&sender];

        match scenario {
            ScenarioType::OnchainNonce => {
                let tx = self
                    .tx_generator
                    .tx(on_chain_nonce, &mut self.rng)
                    .with_gas_price(self.base_fee);
                let valid_tx = self.validator.validated(tx);

                let res = pool.add_transaction(valid_tx, on_chain_balance, on_chain_nonce).unwrap();

                // TODO(mattsse): need a way expect based on the current state of the pool and tx
                // settings

                match res {
                    AddedTransaction::Pending(_) => {}
                    AddedTransaction::Parked { .. } => {
                        panic!("expected pending")
                    }
                }

                // TODO(mattsse): check subpools
            }
            ScenarioType::HigherNonce { .. } => {
                unimplemented!()
            }
        }

        // make sure everything is set
        pool.enforce_invariants()
    }
}

/// How to configure a new mock transaction stream
pub(crate) struct MockSimulatorConfig {
    /// How many senders to generate.
    pub(crate) num_senders: usize,
    /// Scenarios to test
    pub(crate) scenarios: Vec<ScenarioType>,
    /// The start base fee
    pub(crate) base_fee: u128,
    /// generator for transactions
    pub(crate) tx_generator: MockTransactionDistribution,
}

impl MockSimulatorConfig {
    /// Generates a set of random addresses
    pub(crate) fn addresses(&self, rng: &mut impl rand::Rng) -> Vec<Address> {
        std::iter::repeat_with(|| Address::random_with(rng)).take(self.num_senders).collect()
    }
}

/// Represents
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum ScenarioType {
    OnchainNonce,
    HigherNonce { skip: u64 },
}

/// The actual scenario, ready to be executed
///
/// A scenario produces one or more transactions and expects a certain Outcome.
///
/// An executed scenario can affect previous executed transactions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum Scenario {
    /// Send a tx with the same nonce as on chain.
    OnchainNonce { nonce: u64 },
    /// Send a tx with a higher nonce that what the sender has on chain
    HigherNonce { onchain: u64, nonce: u64 },
    Multi {
        // Execute multiple test scenarios
        scenario: Vec<Scenario>,
    },
}

/// Represents an executed scenario
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ExecutedScenario {
    /// balance at the time of execution
    balance: U256,
    /// nonce at the time of execution
    nonce: u64,
    /// The executed scenario
    scenario: Scenario,
}

/// All executed scenarios by a sender
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ExecutedScenarios {
    sender: Address,
    scenarios: Vec<ExecutedScenario>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{MockFeeRange, MockTransactionRatio};

    #[test]
    fn test_on_chain_nonce_scenario() {
        let transaction_ratio = MockTransactionRatio {
            legacy_pct: 30,
            dynamic_fee_pct: 70,
            access_list_pct: 0,
            blob_pct: 0,
        };

        let fee_ranges = MockFeeRange {
            gas_price: (10u128..100).into(),
            priority_fee: (10u128..100).into(),
            max_fee: (100u128..110).into(),
            max_fee_blob: (1u128..100).into(),
        };

        let config = MockSimulatorConfig {
            num_senders: 10,
            scenarios: vec![ScenarioType::OnchainNonce],
            base_fee: 10,
            tx_generator: MockTransactionDistribution::new(
                transaction_ratio,
                fee_ranges,
                10..100,
                10..100,
            ),
        };
        let mut simulator = MockTransactionSimulator::new(rand::thread_rng(), config);
        let mut pool = MockPool::default();

        simulator.next(&mut pool);
    }
}
