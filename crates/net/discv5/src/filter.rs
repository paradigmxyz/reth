//! Predicates to constraint peer lookups.

use derive_more::Constructor;

/// Allows users to inject custom filtering rules on which peers to discover.
pub trait FilterDiscovered {
    /// Applies filtering rules on [`Enr`](discv5::Enr) data. Returns [`Ok`](FilterOutcome::Ok) if
    /// peer should be included, otherwise [`Ignore`](FilterOutcome::Ignore).
    fn filter_discovered_peer(&self, _enr: &discv5::Enr) -> FilterOutcome;

    /// Message for [`FilterOutcome::Ignore`] should specify the reason for filtering out a node
    /// record.
    fn ignore_reason(&self) -> String;
}

/// Filter that lets all [`Enr`](discv5::Enr)s pass through.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopFilter;

impl FilterDiscovered for NoopFilter {
    fn filter_discovered_peer(&self, _enr: &discv5::Enr) -> FilterOutcome {
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
    fn filter_discovered_peer(&self, enr: &discv5::Enr) -> FilterOutcome {
        if enr.get(self.chain).is_none() {
            return FilterOutcome::Ignore { reason: self.ignore_reason() }
        }
        FilterOutcome::Ok
    }

    fn ignore_reason(&self) -> String {
        format!("{} fork required", String::from_utf8_lossy(self.chain))
    }
}

/// Filter requiring that peers advertise belonging to a certain fork.
#[derive(Debug, Constructor, Clone, Copy)]
pub struct MustIncludeFork {
    /// Filters chain which node record must advertise.
    chain: MustIncludeChain,
    /// Fork which node record must advertise.
    fork: &'static [u8],
}

impl FilterDiscovered for MustIncludeFork {
    fn filter_discovered_peer(&self, enr: &discv5::Enr) -> FilterOutcome {
        let Some(fork) = enr.get(self.chain.chain) else {
            return FilterOutcome::Ignore { reason: self.chain.ignore_reason() }
        };

        if fork != self.fork {
            return FilterOutcome::Ignore { reason: self.ignore_reason() }
        }

        FilterOutcome::Ok
    }

    fn ignore_reason(&self) -> String {
        format!(
            "{} fork {} required",
            String::from_utf8_lossy(self.chain.chain),
            String::from_utf8_lossy(self.fork)
        )
    }
}
