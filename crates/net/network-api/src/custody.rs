//! Shared blob cell custody state.

use alloy_primitives::B128;
use std::sync::{
    atomic::{AtomicU64, AtomicU8, Ordering},
    Arc,
};

/// Shared blob cell custody bitmap for [EIP-8070] sparse blobpool sampling.
///
/// This stores the latest `custodyColumns` value received from
/// `engine_forkchoiceUpdatedV4`, so network components can align blob cell sampling with the
/// consensus client's custody set. The value is a lightweight hint for sampling decisions, not a
/// consensus-critical guard.
///
/// [EIP-8070]: https://eips.ethereum.org/EIPS/eip-8070
#[derive(Debug, Clone, Default)]
pub struct CellCustody {
    inner: Arc<CellCustodyInner>,
}

impl CellCustody {
    /// Returns the currently configured blob cell custody bitmap.
    ///
    /// Reads are intentionally cheap and eventually consistent. Concurrent custom-to-custom
    /// updates may briefly expose an in-flight bitmap, which is acceptable because custody is only
    /// used to steer future sampling requests.
    pub fn get(&self) -> B128 {
        match self.inner.state.load(Ordering::Relaxed) {
            CELL_CUSTODY_NONE => B128::from(0u128),
            CELL_CUSTODY_FULL => B128::from(u128::MAX),
            _ => {
                let high = self.inner.high.load(Ordering::Relaxed) as u128;
                let low = self.inner.low.load(Ordering::Relaxed) as u128;

                B128::from((high << 64) | low)
            }
        }
    }

    /// Updates the blob cell custody bitmap.
    pub fn set(&self, custody_columns: B128) {
        let custody_columns = u128::from(custody_columns);
        match custody_columns {
            0 => self.inner.state.store(CELL_CUSTODY_NONE, Ordering::Relaxed),
            u128::MAX => self.inner.state.store(CELL_CUSTODY_FULL, Ordering::Relaxed),
            _ => {
                self.inner.high.store((custody_columns >> 64) as u64, Ordering::Relaxed);
                self.inner.low.store(custody_columns as u64, Ordering::Relaxed);
                self.inner.state.store(CELL_CUSTODY_CUSTOM, Ordering::Relaxed);
            }
        }
    }

    /// Updates the custody bitmap from the Engine API's little-endian bitvector encoding.
    pub fn set_from_engine_api(&self, custody_columns: B128) {
        self.set(B128::from(u128::from_le_bytes(custody_columns.into())));
    }
}

const CELL_CUSTODY_NONE: u8 = 0;
const CELL_CUSTODY_FULL: u8 = 1;
const CELL_CUSTODY_CUSTOM: u8 = 2;

#[derive(Debug, Default)]
struct CellCustodyInner {
    state: AtomicU8,
    high: AtomicU64,
    low: AtomicU64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_engine_api_bitvector_as_numeric_mask() {
        let custody = CellCustody::default();
        custody.set_from_engine_api(B128::from(0b1010u128.to_le_bytes()));

        assert_eq!(custody.get(), B128::from(0b1010u128));
    }
}
