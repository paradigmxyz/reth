use crate::transactions::config::{AnnouncementFilteringPolicy, TransactionPropagationPolicy};
use std::fmt::Debug;

/// A bundle of policies that control the behavior of network components like
/// the [`TransactionsManager`](super::TransactionsManager).
///
/// This trait allows for different collections of policies to be used interchangeably.
pub trait TransactionPolicies: Send + Sync + Debug + 'static {
    /// The type of the policy used for transaction propagation.
    type Propagation: TransactionPropagationPolicy;
    /// The type of the policy used for filtering transaction announcements.
    type Announcement: AnnouncementFilteringPolicy;

    /// Returns a reference to the transaction propagation policy.
    fn propagation_policy(&self) -> &Self::Propagation;

    /// Returns a mutable reference to the transaction propagation policy.
    fn propagation_policy_mut(&mut self) -> &mut Self::Propagation;

    /// Returns a reference to the announcement filtering policy.
    fn announcement_filter(&self) -> &Self::Announcement;
}

/// A container that bundles specific implementations of transaction-related policies,
///
/// This struct implements the [`TransactionPolicies`] trait, providing a complete set of
/// policies required by components like the [`TransactionsManager`](super::TransactionsManager).
/// It holds a specific [`TransactionPropagationPolicy`] and an
/// [`AnnouncementFilteringPolicy`].
#[derive(Debug, Clone, Default)]
pub struct NetworkPolicies<P, A> {
    propagation: P,
    announcement: A,
}

impl<P, A> NetworkPolicies<P, A> {
    /// Creates a new bundle of network policies.
    pub const fn new(propagation: P, announcement: A) -> Self {
        Self { propagation, announcement }
    }

    /// Returns a new `NetworkPolicies` bundle with the `TransactionPropagationPolicy` replaced.
    pub fn with_propagation<NewP>(self, new_propagation: NewP) -> NetworkPolicies<NewP, A>
    where
        NewP: TransactionPropagationPolicy,
    {
        NetworkPolicies::new(new_propagation, self.announcement)
    }

    /// Returns a new `NetworkPolicies` bundle with the `AnnouncementFilteringPolicy` replaced.
    pub fn with_announcement<NewA>(self, new_announcement: NewA) -> NetworkPolicies<P, NewA>
    where
        NewA: AnnouncementFilteringPolicy,
    {
        NetworkPolicies::new(self.propagation, new_announcement)
    }
}

impl<P, A> TransactionPolicies for NetworkPolicies<P, A>
where
    P: TransactionPropagationPolicy + Debug,
    A: AnnouncementFilteringPolicy + Debug,
{
    type Propagation = P;
    type Announcement = A;

    fn propagation_policy(&self) -> &Self::Propagation {
        &self.propagation
    }

    fn propagation_policy_mut(&mut self) -> &mut Self::Propagation {
        &mut self.propagation
    }

    fn announcement_filter(&self) -> &Self::Announcement {
        &self.announcement
    }
}
