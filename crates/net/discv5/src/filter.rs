//! Predicates to constraint peer lookups.

use dashmap::DashSet;
use derive_more::Constructor;
use itertools::Itertools;

use crate::config::{ETH, ETH2};

/// Outcome of applying filtering rules on node record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterOutcome {
    /// ENR passes filter rules.
    Ok,
    /// ENR doesn't pass filter rules, for the given reason.
    Ignore {
        /// Reason for filtering out node record.
        reason: String,
    },
}

impl FilterOutcome {
    /// Returns `true` for [`FilterOutcome::Ok`].
    pub fn is_ok(&self) -> bool {
        matches!(self, FilterOutcome::Ok)
    }
}

/// Filter requiring that peers advertise that they belong to some fork of a certain chain.
#[derive(Debug, Constructor, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MustIncludeKey {
    /// Kv-pair key which node record must advertise.
    key: &'static [u8],
}

impl MustIncludeKey {
    /// Returns [`FilterOutcome::Ok`] if [`Enr`](discv5::Enr) contains the configured kv-pair key.
    pub fn filter(&self, enr: &discv5::Enr) -> FilterOutcome {
        if enr.get_raw_rlp(self.key).is_none() {
            return FilterOutcome::Ignore { reason: self.ignore_reason() }
        }
        FilterOutcome::Ok
    }

    fn ignore_reason(&self) -> String {
        format!("{} fork required", String::from_utf8_lossy(self.key))
    }
}

/// Filter requiring that peers not advertise that they belong to some chains.
#[derive(Debug, Clone, Default)]
pub struct MustNotIncludeKeys {
    chains: DashSet<MustIncludeKey>,
}

impl MustNotIncludeKeys {
    /// Returns a new instance that disallows node records with a kv-pair that has any of the given
    /// chains as key.
    pub fn new(disallow_chains: &[&'static [u8]]) -> Self {
        let chains = DashSet::with_capacity(disallow_chains.len());
        for chain in disallow_chains {
            _ = chains.insert(MustIncludeKey::new(chain));
        }

        MustNotIncludeKeys { chains }
    }
}

impl MustNotIncludeKeys {
    /// Returns `true` if [`Enr`](discv5::Enr) passes filtering rules.
    pub fn filter(&self, enr: &discv5::Enr) -> FilterOutcome {
        for chain in self.chains.iter() {
            if matches!(chain.filter(enr), FilterOutcome::Ok) {
                return FilterOutcome::Ignore { reason: self.ignore_reason() }
            }
        }

        FilterOutcome::Ok
    }

    fn ignore_reason(&self) -> String {
        format!(
            "{} forks not allowed",
            self.chains.iter().map(|chain| String::from_utf8_lossy(chain.key)).format(",")
        )
    }

    /// Adds a key that must not be present for any kv-pair in a node record.
    pub fn add_disallowed_chains(&mut self, keys: &[&'static [u8]]) {
        for key in keys {
            self.chains.insert(MustIncludeKey::new(*key));
        }
    }
}

#[cfg(test)]
mod tests {
    use alloy_rlp::Bytes;
    use discv5::enr::{CombinedKey, Enr};

    use super::*;

    #[test]
    fn must_not_include_chain_filter() {
        // rig test

        let filter = MustNotIncludeKeys::new(&[ETH, ETH2]);

        // enr_1 advertises a fork from one of the chains configured in filter
        let sk = CombinedKey::generate_secp256k1();
        let enr_1 =
            Enr::builder().add_value_rlp(ETH as &[u8], Bytes::from("cancun")).build(&sk).unwrap();

        // enr_2 advertises a fork from one the other chain configured in filter
        let sk = CombinedKey::generate_secp256k1();
        let enr_2 = Enr::builder().add_value_rlp(ETH2, Bytes::from("deneb")).build(&sk).unwrap();

        // test

        assert!(matches!(filter.filter(&enr_1), FilterOutcome::Ignore { .. }));
        assert!(matches!(filter.filter(&enr_2), FilterOutcome::Ignore { .. }));
    }
}
