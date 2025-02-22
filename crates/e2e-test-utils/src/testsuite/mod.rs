//! Utilities for running e2e tests against a node or a network of nodes.

use std::future::Future;

/// A runner performs operations on an environment.
pub struct Runner<I> {
    env: Environment<I>,
}

impl<I> Runner<I> {

    /// Execute an action
    pub async fn execute<A>(&self, action: A) -> eyre::Result<()>
    where A: Action<I>
    {

        todo!()
    }

}

/// Represents a test environment.
#[derive(Debug, Clone)]
pub struct Environment<I> {
   /// The instance against which we can run tests.
   pub instance: I,
   pub ctx: (),
}

pub enum ActionKind<A> {

}

/// An action can be performed on an instance.
///
/// An action can result in more actions
pub trait Action<I> {

    /// Executes the action
    fn execute(self, env: Environment<I>) -> impl Future<Output = eyre::Result<ActionKind<I>>> + Send;

}