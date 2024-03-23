//! Predicates to constraint peer lookups.

use alloy_rlp::Decodable;
use derive_more::Constructor;
use reth_primitives::{ForkId, MAINNET};

use crate::{ChainRef, IdentifyForkIdKVPair};

/// Allows users to inject custom filtering rules on which peers to discover.
pub trait FilterDiscovered {
    /// Applies filtering rules on [`Enr`](discv5::Enr) data. Returns [`Ok`](FilterOutcome::Ok) if
    /// peer should be included, otherwise [`Ignore`](FilterOutcome::Ignore).
    fn filter(&self, enr: &discv5::Enr) -> FilterOutcome;

    /// Message for [`FilterOutcome::Ignore`] should specify the reason for filtering out a node
    /// record.
    fn ignore_reason(&self) -> String;
}

/// Filter that lets all [`Enr`](discv5::Enr)s pass through.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopFilter;

impl FilterDiscovered for NoopFilter {
    fn filter(&self, _enr: &discv5::Enr) -> FilterOutcome {
        FilterOutcome::Ok
    }

    fn ignore_reason(&self) -> String {
        unreachable!()
    }
}

/// Outcome of applying filtering rules on node record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterOutcome {
    /// ENR passes filter rules.
    Ok,
    /// ENR passes filter rules. [`ForkId`] is a by-product of filtering, and is returned to avoid
    /// rlp decoding it twice.
    OkReturnForkId(ForkId),
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
#[derive(Debug, Constructor, Clone, Copy)]
pub struct MustIncludeChain {
    /// Chain which node record must advertise.
    chain: &'static [u8],
}

impl FilterDiscovered for MustIncludeChain {
    fn filter(&self, enr: &discv5::Enr) -> FilterOutcome {
        if enr.get_raw_rlp(self.chain).is_none() {
            return FilterOutcome::Ignore { reason: self.ignore_reason() }
        }
        FilterOutcome::Ok
    }

    fn ignore_reason(&self) -> String {
        format!("{} fork required", String::from_utf8_lossy(self.chain))
    }
}

impl Default for MustIncludeChain {
    fn default() -> Self {
        Self { chain: ChainRef::ETH }
    }
}

/// Filter requiring that peers advertise belonging to a certain fork.
#[derive(Debug, Clone)]
pub struct MustIncludeFork {
    /// Filters chain which node record must advertise.
    chain: MustIncludeChain,
    /// Fork which node record must advertise.
    fork_id: ForkId,
}

impl MustIncludeFork {
    /// Returns a new instance.
    pub fn new(chain: &'static [u8], fork_id: ForkId) -> Self {
        Self { chain: MustIncludeChain::new(chain), fork_id }
    }
}

impl FilterDiscovered for MustIncludeFork {
    fn filter(&self, enr: &discv5::Enr) -> FilterOutcome {
        let Some(mut fork_id_bytes) = enr.get_raw_rlp(self.chain.chain) else {
            return FilterOutcome::Ignore { reason: self.chain.ignore_reason() }
        };

        if let Ok(fork_id) = ForkId::decode(&mut fork_id_bytes) {
            if fork_id == self.fork_id {
                return FilterOutcome::OkReturnForkId(fork_id)
            }
        }

        FilterOutcome::Ignore { reason: self.ignore_reason() }
    }

    fn ignore_reason(&self) -> String {
        format!("{} fork {:?} required", String::from_utf8_lossy(self.chain.chain), self.fork_id)
    }
}

impl Default for MustIncludeFork {
    fn default() -> Self {
        Self { chain: MustIncludeChain::new(ChainRef::ETH), fork_id: MAINNET.latest_fork_id() }
    }
}

#[cfg(test)]
mod tests {
    use discv5::enr::{CombinedKey, Enr};

    use super::*;

    #[test]
    fn fork_filter() {
        // rig test

        let fork = MAINNET.cancun_fork_id().unwrap();
        let filter = MustIncludeFork::new(b"eth", fork);

        // enr_1 advertises fork configured in filter
        let sk = CombinedKey::generate_secp256k1();
        let enr_1 = Enr::builder()
            .add_value_rlp(ChainRef::ETH as &[u8], alloy_rlp::encode(fork).into())
            .build(&sk)
            .unwrap();

        // enr_2 advertises an older fork
        let sk = CombinedKey::generate_secp256k1();
        let enr_2 = Enr::builder()
            .add_value_rlp(
                ChainRef::ETH,
                alloy_rlp::encode(MAINNET.shanghai_fork_id().unwrap()).into(),
            )
            .build(&sk)
            .unwrap();

        // test

        assert!(matches!(filter.filter(&enr_1), FilterOutcome::OkReturnForkId(_)));
        assert!(matches!(filter.filter(&enr_2), FilterOutcome::Ignore { .. }));
    }
}
