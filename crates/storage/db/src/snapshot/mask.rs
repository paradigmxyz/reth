use crate::table::Decompress;

/// Helper struct to select column values from a snapshot row
#[derive(Debug)]
pub struct Mask<T, J = (), K = ()>(std::marker::PhantomData<(T, J, K)>);

/// Mask for Header snapshot segment.
pub type HeaderMask<T, J = (), K = ()> = Mask<T, J, K>;

/// Mask for Receipt snapshot segment.
pub type ReceiptMask<T, J = (), K = ()> = Mask<T, J, K>;

/// Mask for Transaction snapshot segment.
pub type TransactionMask<T, J = (), K = ()> = Mask<T, J, K>;

/// Column mask for one value.
pub trait ColumnMaskOne {
    /// First column value
    type T: Decompress;
    /// Mask to obtain desired values. Should be in the same order as snapshot creation.
    const MASK: usize;
}

/// Column mask for two values.
pub trait ColumnMaskTwo {
    /// First column value
    type T: Decompress;
    /// Second column value
    type J: Decompress;
    /// Mask to obtain desired values. Should be in the same order as snapshot creation.
    const MASK: usize;
}

/// Column mask for three values.
pub trait ColumnMaskThree {
    /// First column value
    type T: Decompress;
    /// Second column value
    type J: Decompress;
    /// Third column value
    type K: Decompress;
    /// Mask to obtain desired values. Should be in the same order as snapshot creation.
    const MASK: usize;
}

#[macro_export]
/// Add snapshot mask
macro_rules! add_snapshot_mask {
    ($mask_struct:tt, $type1:ty, $mask:expr) => {
        impl ColumnMaskOne for $mask_struct<$type1> {
            type T = $type1;
            const MASK: usize = $mask;
        }
    };
    ($mask_struct:tt, $type1:ty, $type2:ty, $mask:expr) => {
        impl ColumnMaskTwo for $mask_struct<$type1, $type2> {
            type T = $type1;
            type J = $type2;
            const MASK: usize = $mask;
        }
    };
    ($mask_struct:tt, $type1:ty, $type2:ty, $type3:ty, $mask:expr) => {
        impl ColumnMaskTwo for $mask_struct<$type1, $type2, $type3> {
            type T = $type1;
            type J = $type2;
            type K = $type3;
            const MASK: usize = $mask;
        }
    };
}
