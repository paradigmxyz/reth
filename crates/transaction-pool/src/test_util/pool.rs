//! Test helpers for mocking an entire pool.

use crate::{
    pool::txpool::TxPool,
    test_util::{MockOrdering, MockTransactionDistribution},
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
pub struct MockPool<T: TransactionOrdering = MockOrdering> {
    // The wrapped pool.
    pool: TxPool<T>,
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
pub struct MockTransactionSimulator<R: Rng> {
    /// The pending base fee
    base_fee: U256,
    /// pending gas price
    gas_price: U256,
    /// Generator for transactions
    tx_generator: MockTransactionDistribution,
    /// represents the on chain balance of a sender.
    balances: HashMap<Address, U256>,
    /// represents the on chain nonce of a sender.
    nonces: HashMap<Address, u64>,
    /// A set of addresses to as senders.
    senders: Vec<Address>,
    /// What scenarios to execute
    scenarios: Vec<ScenarioType>,
    /// All previous scenarios executed by a sender
    executed: HashMap<Address, ExecutedScenarios>,
    /// The rng instance used to select senders and scenarios.
    rng: R,
}

impl<R: Rng> MockTransactionSimulator<R> {
    /// Returns a new mock instance
    pub fn new(mut rng: R, config: MockSimulatorConfig) -> Self {
        let senders = config.addresses(&mut rng);
        let nonces = senders.iter().copied().map(|a| (a, 0)).collect();
        let balances = senders.iter().copied().map(|a| (a, config.balance)).collect();
        Self {
            base_fee: Default::default(),
            gas_price: config.gas_price,
            balances,
            nonces,
            senders,
            scenarios: config.scenarios,
            tx_generator: config.tx_generator,
            executed: Default::default(),
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
    pub fn next(&mut self, pool: &mut MockPool) {
        let sender = self.rng_address();
        let scenario = self.rng_scenario();
        let on_chain_nonce = self.nonces[&sender];
        let on_chain_balance = self.balances[&sender];

        match scenario {
            ScenarioType::OnchainNonce => {}
            ScenarioType::HigherNonce { .. } => {
                unimplemented!()
            }
        }
    }
}

/// How to configure a new mock transaction stream
pub struct MockSimulatorConfig {
    /// How many senders to generate.
    pub num_senders: usize,
    // TODO(mattsse): add a way to generate different balances
    pub balance: U256,
    /// Scenarios to test
    pub scenarios: Vec<ScenarioType>,
    /// The start gas price
    pub gas_price: U256,
    /// The start base fee
    pub base_fee: U256,
    /// generator for transactions
    pub tx_generator: MockTransactionDistribution,
}

impl MockSimulatorConfig {
    /// Generates a set of random addresses
    pub fn addresses(&self, rng: &mut impl rand::Rng) -> Vec<Address> {
        std::iter::repeat_with(|| Address::random_using(rng)).take(self.num_senders).collect()
    }
}

/// Represents
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ScenarioType {
    OnchainNonce,
    HigherNonce { skip: u64 },
}

/// The actual scenario, ready to be executed
///
/// A scenario produces one or more transactions and expects a certain Outcome.
///
/// An executed scenario can affect previous executed transactions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Scenario {
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
pub struct ExecutedScenario {
    /// balance at the time of execution
    balance: U256,
    /// nonce at the time of execution
    nonce: u64,
    /// The executed scenario
    scenario: Scenario,
}

/// All executed scenarios by a sender
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutedScenarios {
    sender: Address,
    scenarios: Vec<ExecutedScenario>,
}

#[test]
fn test_on_chain_nonce_scenario() {
    let config = MockSimulatorConfig {
        num_senders: 10,
        balance: 100_000u64.into(),
        scenarios: vec![ScenarioType::OnchainNonce],
        gas_price: Default::default(),
        base_fee: Default::default(),
        tx_generator: MockTransactionDistribution::new(30, 10..1_000),
    };
}
