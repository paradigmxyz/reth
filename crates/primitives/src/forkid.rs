//! EIP-2124 implementation based on <https://eips.ethereum.org/EIPS/eip-2124>.
//! Previously version of apache licenced: https://crates.io/crates/ethereum-forkid

#![deny(missing_docs)]

use crate::{BlockNumber, H256};
use crc::crc32;
use reth_rlp::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    ops::{Add, AddAssign},
};
use thiserror::Error;

/// `CRC32` hash of all previous forks starting from genesis block.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    RlpEncodableWrapper,
    RlpDecodableWrapper,
    RlpMaxEncodedLen,
    Serialize,
    Deserialize,
)]
pub struct ForkHash(pub [u8; 4]);

impl fmt::Debug for ForkHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ForkHash").field(&hex::encode(&self.0[..])).finish()
    }
}

impl From<H256> for ForkHash {
    fn from(genesis: H256) -> Self {
        Self(crc32::checksum_ieee(&genesis[..]).to_be_bytes())
    }
}

impl AddAssign<BlockNumber> for ForkHash {
    fn add_assign(&mut self, block: BlockNumber) {
        let blob = block.to_be_bytes();
        self.0 = crc32::update(u32::from_be_bytes(self.0), &crc32::IEEE_TABLE, &blob).to_be_bytes();
    }
}

impl Add<BlockNumber> for ForkHash {
    type Output = Self;
    fn add(mut self, block: BlockNumber) -> Self {
        self += block;
        self
    }
}

/// A fork identifier as defined by EIP-2124.
/// Serves as the chain compatibility identifier.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    RlpEncodable,
    RlpDecodable,
    RlpMaxEncodedLen,
    Serialize,
    Deserialize,
)]
pub struct ForkId {
    /// CRC32 checksum of the all fork blocks from genesis.
    pub hash: ForkHash,
    /// Next upcoming fork block number, 0 if not yet known.
    pub next: BlockNumber,
}

/// Reason for rejecting provided `ForkId`.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq, Hash)]
pub enum ValidationError {
    /// Remote node is outdated and needs a software update.
    #[error(
        "remote node is outdated and needs a software update: local={local:?}, remote={remote:?}"
    )]
    RemoteStale {
        /// locally configured forkId
        local: ForkId,
        /// ForkId received from remote
        remote: ForkId,
    },
    /// Local node is on an incompatible chain or needs a software update.
    #[error("local node is on an incompatible chain or needs a software update: local={local:?}, remote={remote:?}")]
    LocalIncompatibleOrStale {
        /// locally configured forkId
        local: ForkId,
        /// ForkId received from remote
        remote: ForkId,
    },
}

/// Filter that describes the state of blockchain and can be used to check incoming `ForkId`s for
/// compatibility.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForkFilter {
    forks: BTreeMap<BlockNumber, ForkHash>,

    head: BlockNumber,

    cache: Cache,
}

impl ForkFilter {
    /// Create the filter from provided head, genesis block hash, past forks and expected future
    /// forks.
    pub fn new<F, B>(head: BlockNumber, genesis: H256, forks: F) -> Self
    where
        F: IntoIterator<Item = B>,
        B: Into<BlockNumber>,
    {
        let genesis_fork_hash = ForkHash::from(genesis);
        let mut forks = forks.into_iter().map(Into::into).collect::<BTreeSet<_>>();
        forks.remove(&0);

        let forks = forks
            .into_iter()
            .fold(
                (BTreeMap::from([(0, genesis_fork_hash)]), genesis_fork_hash),
                |(mut acc, base_hash), block| {
                    let fork_hash = base_hash + block;
                    acc.insert(block, fork_hash);
                    (acc, fork_hash)
                },
            )
            .0;

        let cache = Cache::compute_cache(&forks, head);

        Self { forks, head, cache }
    }

    fn set_head_priv(&mut self, head: BlockNumber) -> bool {
        let recompute_cache = {
            if head < self.cache.epoch_start {
                true
            } else if let Some(epoch_end) = self.cache.epoch_end {
                head >= epoch_end
            } else {
                false
            }
        };

        if recompute_cache {
            self.cache = Cache::compute_cache(&self.forks, head);
        }
        self.head = head;

        recompute_cache
    }

    /// Set the current head
    pub fn set_head(&mut self, head: BlockNumber) {
        self.set_head_priv(head);
    }

    /// Return current fork id
    #[must_use]
    pub const fn current(&self) -> ForkId {
        self.cache.fork_id
    }

    /// Check whether the provided `ForkId` is compatible based on the validation rules in
    /// `EIP-2124`.
    ///
    /// Implements the rules following: <https://github.com/ethereum/EIPs/blob/master/EIPS/eip-2124.md#stale-software-examples>
    ///
    /// # Errors
    ///
    /// Returns a `ValidationError` if the `ForkId` is not compatible.
    pub fn validate(&self, fork_id: ForkId) -> Result<(), ValidationError> {
        // 1) If local and remote FORK_HASH matches...
        if self.current().hash == fork_id.hash {
            if fork_id.next == 0 {
                // 1b) No remotely announced fork, connect.
                return Ok(())
            }

            //... compare local head to FORK_NEXT.
            return if self.head >= fork_id.next {
                // 1a) A remotely announced but remotely not passed block is already passed locally,
                // disconnect, since the chains are incompatible.
                Err(ValidationError::LocalIncompatibleOrStale {
                    local: self.current(),
                    remote: fork_id,
                })
            } else {
                // 1b) Remotely announced fork not yet passed locally, connect.
                Ok(())
            }
        }

        // 2) If the remote FORK_HASH is a subset of the local past forks...
        let mut it = self.cache.past.iter();
        while let Some((_, hash)) = it.next() {
            if *hash == fork_id.hash {
                // ...and the remote FORK_NEXT matches with the locally following fork block number,
                // connect.
                if let Some((actual_fork_block, _)) = it.next() {
                    return if *actual_fork_block == fork_id.next {
                        Ok(())
                    } else {
                        Err(ValidationError::RemoteStale { local: self.current(), remote: fork_id })
                    }
                }

                break
            }
        }

        // 3) If the remote FORK_HASH is a superset of the local past forks and can be completed
        // with locally known future forks, connect.
        for future_fork_hash in &self.cache.future {
            if *future_fork_hash == fork_id.hash {
                return Ok(())
            }
        }

        // 4) Reject in all other cases.
        Err(ValidationError::LocalIncompatibleOrStale { local: self.current(), remote: fork_id })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Cache {
    // An epoch is a period between forks.
    // When we progress from one fork to the next one we move to the next epoch.
    epoch_start: BlockNumber,
    epoch_end: Option<BlockNumber>,
    past: Vec<(BlockNumber, ForkHash)>,
    future: Vec<ForkHash>,
    fork_id: ForkId,
}

impl Cache {
    /// Compute cache.
    fn compute_cache(forks: &BTreeMap<BlockNumber, ForkHash>, head: BlockNumber) -> Self {
        let mut past = Vec::with_capacity(forks.len());
        let mut future = Vec::with_capacity(forks.len());

        let mut epoch_start = 0;
        let mut epoch_end = None;
        for (block, hash) in forks {
            if *block <= head {
                epoch_start = *block;
                past.push((*block, *hash));
            } else {
                if epoch_end.is_none() {
                    epoch_end = Some(*block);
                }
                future.push(*hash);
            }
        }

        let fork_id = ForkId {
            hash: past.last().expect("there is always at least one - genesis - fork hash; qed").1,
            next: epoch_end.unwrap_or(0),
        };

        Self { epoch_start, epoch_end, past, future, fork_id }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_literal::hex;
    const GENESIS_HASH: H256 =
        H256(hex!("d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3"));

    // EIP test vectors.

    #[test]
    fn forkhash() {
        let mut fork_hash = ForkHash::from(GENESIS_HASH);
        assert_eq!(fork_hash.0, hex!("fc64ec04"));

        fork_hash += 1_150_000;
        assert_eq!(fork_hash.0, hex!("97c2c34c"));

        fork_hash += 1_920_000;
        assert_eq!(fork_hash.0, hex!("91d1f948"));
    }

    #[test]
    fn compatibility_check() {
        let mut filter = ForkFilter::new(
            0,
            GENESIS_HASH,
            vec![1_150_000u64, 1_920_000, 2_463_000, 2_675_000, 4_370_000, 7_280_000],
        );

        // Local is mainnet Petersburg, remote announces the same. No future fork is announced.
        filter.set_head(7_987_396);
        assert_eq!(filter.validate(ForkId { hash: ForkHash(hex!("668db0af")), next: 0 }), Ok(()));

        // Local is mainnet Petersburg, remote announces the same. Remote also announces a next fork
        // at block 0xffffffff, but that is uncertain.
        filter.set_head(7_987_396);
        assert_eq!(
            filter.validate(ForkId { hash: ForkHash(hex!("668db0af")), next: BlockNumber::MAX }),
            Ok(())
        );

        // Local is mainnet currently in Byzantium only (so it's aware of Petersburg),remote
        // announces also Byzantium, but it's not yet aware of Petersburg (e.g. non updated
        // node before the fork). In this case we don't know if Petersburg passed yet or
        // not.
        filter.set_head(7_279_999);
        assert_eq!(filter.validate(ForkId { hash: ForkHash(hex!("a00bc324")), next: 0 }), Ok(()));

        // Local is mainnet currently in Byzantium only (so it's aware of Petersburg), remote
        // announces also Byzantium, and it's also aware of Petersburg (e.g. updated node
        // before the fork). We don't know if Petersburg passed yet (will pass) or not.
        filter.set_head(7_279_999);
        assert_eq!(
            filter.validate(ForkId { hash: ForkHash(hex!("a00bc324")), next: 7_280_000 }),
            Ok(())
        );

        // Local is mainnet currently in Byzantium only (so it's aware of Petersburg), remote
        // announces also Byzantium, and it's also aware of some random fork (e.g.
        // misconfigured Petersburg). As neither forks passed at neither nodes, they may
        // mismatch, but we still connect for now.
        filter.set_head(7_279_999);
        assert_eq!(
            filter.validate(ForkId { hash: ForkHash(hex!("a00bc324")), next: BlockNumber::MAX }),
            Ok(())
        );

        // Local is mainnet Petersburg, remote announces Byzantium + knowledge about Petersburg.
        // Remote is simply out of sync, accept.
        filter.set_head(7_987_396);
        assert_eq!(
            filter.validate(ForkId { hash: ForkHash(hex!("a00bc324")), next: 7_280_000 }),
            Ok(())
        );

        // Local is mainnet Petersburg, remote announces Spurious + knowledge about Byzantium.
        // Remote is definitely out of sync. It may or may not need the Petersburg update,
        // we don't know yet.
        filter.set_head(7_987_396);
        assert_eq!(
            filter.validate(ForkId { hash: ForkHash(hex!("3edd5b10")), next: 4_370_000 }),
            Ok(())
        );

        // Local is mainnet Byzantium, remote announces Petersburg. Local is out of sync, accept.
        filter.set_head(7_279_999);
        assert_eq!(filter.validate(ForkId { hash: ForkHash(hex!("668db0af")), next: 0 }), Ok(()));

        // Local is mainnet Spurious, remote announces Byzantium, but is not aware of Petersburg.
        // Local out of sync. Local also knows about a future fork, but that is uncertain
        // yet.
        filter.set_head(4_369_999);
        assert_eq!(filter.validate(ForkId { hash: ForkHash(hex!("a00bc324")), next: 0 }), Ok(()));

        // Local is mainnet Petersburg. remote announces Byzantium but is not aware of further
        // forks. Remote needs software update.
        filter.set_head(7_987_396);
        let remote = ForkId { hash: ForkHash(hex!("a00bc324")), next: 0 };
        assert_eq!(
            filter.validate(remote),
            Err(ValidationError::RemoteStale { local: filter.current(), remote })
        );

        // Local is mainnet Petersburg, and isn't aware of more forks. Remote announces Petersburg +
        // 0xffffffff. Local needs software update, reject.
        filter.set_head(7_987_396);
        let remote = ForkId { hash: ForkHash(hex!("5cddc0e1")), next: 0 };
        assert_eq!(
            filter.validate(remote),
            Err(ValidationError::LocalIncompatibleOrStale { local: filter.current(), remote })
        );

        // Local is mainnet Byzantium, and is aware of Petersburg. Remote announces Petersburg +
        // 0xffffffff. Local needs software update, reject.
        filter.set_head(7_279_999);
        let remote = ForkId { hash: ForkHash(hex!("5cddc0e1")), next: 0 };
        assert_eq!(
            filter.validate(remote),
            Err(ValidationError::LocalIncompatibleOrStale { local: filter.current(), remote })
        );

        // Local is mainnet Petersburg, remote is Rinkeby Petersburg.
        filter.set_head(7_987_396);
        let remote = ForkId { hash: ForkHash(hex!("afec6b27")), next: 0 };
        assert_eq!(
            filter.validate(remote),
            Err(ValidationError::LocalIncompatibleOrStale { local: filter.current(), remote })
        );

        // Local is mainnet Petersburg, far in the future. Remote announces Gopherium (non existing
        // fork) at some future block 88888888, for itself, but past block for local. Local
        // is incompatible.
        //
        // This case detects non-upgraded nodes with majority hash power (typical Ropsten mess).
        filter.set_head(88_888_888);
        let remote = ForkId { hash: ForkHash(hex!("668db0af")), next: 88_888_888 };
        assert_eq!(
            filter.validate(remote),
            Err(ValidationError::LocalIncompatibleOrStale { local: filter.current(), remote })
        );

        // Local is mainnet Byzantium. Remote is also in Byzantium, but announces Gopherium (non
        // existing fork) at block 7279999, before Petersburg. Local is incompatible.
        filter.set_head(7_279_999);
        let remote = ForkId { hash: ForkHash(hex!("a00bc324")), next: 7_279_999 };
        assert_eq!(
            filter.validate(remote),
            Err(ValidationError::LocalIncompatibleOrStale { local: filter.current(), remote })
        );
    }

    #[test]
    fn forkid_serialization() {
        assert_eq!(
            &*encode_fixed_size(&ForkId { hash: ForkHash(hex!("00000000")), next: 0 }),
            hex!("c6840000000080")
        );
        assert_eq!(
            &*encode_fixed_size(&ForkId { hash: ForkHash(hex!("deadbeef")), next: 0xBADD_CAFE }),
            hex!("ca84deadbeef84baddcafe")
        );
        assert_eq!(
            &*encode_fixed_size(&ForkId { hash: ForkHash(hex!("ffffffff")), next: u64::MAX }),
            hex!("ce84ffffffff88ffffffffffffffff")
        );

        assert_eq!(
            ForkId::decode(&mut (&hex!("c6840000000080") as &[u8])).unwrap(),
            ForkId { hash: ForkHash(hex!("00000000")), next: 0 }
        );
        assert_eq!(
            ForkId::decode(&mut (&hex!("ca84deadbeef84baddcafe") as &[u8])).unwrap(),
            ForkId { hash: ForkHash(hex!("deadbeef")), next: 0xBADD_CAFE }
        );
        assert_eq!(
            ForkId::decode(&mut (&hex!("ce84ffffffff88ffffffffffffffff") as &[u8])).unwrap(),
            ForkId { hash: ForkHash(hex!("ffffffff")), next: u64::MAX }
        );
    }

    #[test]
    fn compute_cache() {
        let b1 = 1_150_000;
        let b2 = 1_920_000;

        let h0 = ForkId { hash: ForkHash(hex!("fc64ec04")), next: b1 };
        let h1 = ForkId { hash: ForkHash(hex!("97c2c34c")), next: b2 };
        let h2 = ForkId { hash: ForkHash(hex!("91d1f948")), next: 0 };

        let mut fork_filter = ForkFilter::new(0, GENESIS_HASH, vec![b1, b2]);

        assert!(!fork_filter.set_head_priv(0));
        assert_eq!(fork_filter.current(), h0);

        assert!(!fork_filter.set_head_priv(1));
        assert_eq!(fork_filter.current(), h0);

        assert!(fork_filter.set_head_priv(b1 + 1));
        assert_eq!(fork_filter.current(), h1);

        assert!(!fork_filter.set_head_priv(b1));
        assert_eq!(fork_filter.current(), h1);

        assert!(fork_filter.set_head_priv(b1 - 1));
        assert_eq!(fork_filter.current(), h0);

        assert!(fork_filter.set_head_priv(b1));
        assert_eq!(fork_filter.current(), h1);

        assert!(!fork_filter.set_head_priv(b2 - 1));
        assert_eq!(fork_filter.current(), h1);

        assert!(fork_filter.set_head_priv(b2));
        assert_eq!(fork_filter.current(), h2);
    }
}
