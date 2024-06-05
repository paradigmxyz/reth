//! Predicates to constraint peer lookups.

use std::collections::HashSet;

use derive_more::Constructor;
use itertools::Itertools;

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
    pub const fn is_ok(&self) -> bool {
        matches!(self, Self::Ok)
    }
}

/// Filter requiring that peers advertise that they belong to some fork of a certain key.
#[derive(Debug, Constructor, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MustIncludeKey {
    /// Kv-pair key which node record must advertise.
    key: &'static [u8],
}

impl MustIncludeKey {
    /// Returns [`FilterOutcome::Ok`] if [`Enr`](discv5::Enr) contains the configured kv-pair key.
    pub fn filter(&self, enr: &discv5::Enr) -> FilterOutcome {
        if enr.get_raw_rlp(self.key).is_none() {
            return FilterOutcome::Ignore {
                reason: format!("{} fork required", String::from_utf8_lossy(self.key)),
            }
        }
        FilterOutcome::Ok
    }
}

/// Filter requiring that peers not advertise kv-pairs using certain keys, e.g. b"eth2".
#[derive(Debug, Clone, Default)]
pub struct MustNotIncludeKeys {
    keys: HashSet<MustIncludeKey>,
}

impl MustNotIncludeKeys {
    /// Returns a new instance that disallows node records with a kv-pair that has any of the given
    /// keys.
    pub fn new(disallow_keys: &[&'static [u8]]) -> Self {
        let mut keys = HashSet::with_capacity(disallow_keys.len());
        for key in disallow_keys {
            _ = keys.insert(MustIncludeKey::new(key));
        }

        Self { keys }
    }
}

impl MustNotIncludeKeys {
    /// Returns `true` if [`Enr`](discv5::Enr) passes filtering rules.
    pub fn filter(&self, enr: &discv5::Enr) -> FilterOutcome {
        for key in &self.keys {
            if matches!(key.filter(enr), FilterOutcome::Ok) {
                return FilterOutcome::Ignore {
                    reason: format!(
                        "{} forks not allowed",
                        self.keys.iter().map(|key| String::from_utf8_lossy(key.key)).format(",")
                    ),
                }
            }
        }

        FilterOutcome::Ok
    }

    /// Adds a key that must not be present for any kv-pair in a node record.
    pub fn add_disallowed_keys(&mut self, keys: &[&'static [u8]]) {
        for key in keys {
            self.keys.insert(MustIncludeKey::new(key));
        }
    }
}

#[cfg(test)]
mod tests {
    use alloy_rlp::Bytes;
    use discv5::enr::{CombinedKey, Enr};

    use crate::NetworkStackId;

    use super::*;

    #[test]
    fn must_not_include_key_filter() {
        // rig test

        let filter = MustNotIncludeKeys::new(&[NetworkStackId::ETH, NetworkStackId::ETH2]);

        // enr_1 advertises a fork from one of the keys configured in filter
        let sk = CombinedKey::generate_secp256k1();
        let enr_1 = Enr::builder()
            .add_value_rlp(NetworkStackId::ETH as &[u8], Bytes::from("cancun"))
            .build(&sk)
            .unwrap();

        // enr_2 advertises a fork from one the other key configured in filter
        let sk = CombinedKey::generate_secp256k1();
        let enr_2 = Enr::builder()
            .add_value_rlp(NetworkStackId::ETH2, Bytes::from("deneb"))
            .build(&sk)
            .unwrap();

        // test

        assert!(matches!(filter.filter(&enr_1), FilterOutcome::Ignore { .. }));
        assert!(matches!(filter.filter(&enr_2), FilterOutcome::Ignore { .. }));
    }
}
