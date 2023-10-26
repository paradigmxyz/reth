use crate::table::Decompress;

/// Generic Mask helper struct for selecting specific column values to read and decompress.
///
/// #### Explanation:
///
/// A `NippyJar` snapshot row can contain multiple column values. To specify the column values
/// to be read, a mask is utilized.
///
/// For example, a snapshot with three columns, if the first and last columns are queried, the mask
/// `0b101` would be passed. To select only the second column, the mask `0b010` would be used.
///
/// Since each snapshot has its own column distribution, different wrapper types are necessary. For
/// instance, `B256` might be the third column in the `Header` segment, while being the second
/// column in another segment. Hence, `Mask<B256>` would only be applicable to one of these
/// scenarios.
///
/// Alongside, the column selector traits (eg. [`ColumnSelectorOne`]) this provides a structured way
/// to tie the types to be decoded to the mask necessary to query them.
#[derive(Debug)]
pub struct Mask<T, J = (), K = ()>(std::marker::PhantomData<(T, J, K)>);

macro_rules! add_segments {
    ($($segment:tt),+) => {
        paste::paste! {
            $(
                #[doc = concat!("Mask for ", stringify!($segment), " snapshot segment. See [`Mask`] for more.")]
                #[derive(Debug)]
                pub struct [<$segment Mask>]<T, J = (), K = ()>(Mask<T, J, K>);
            )+
        }
    };
}
add_segments!(Header, Receipt, Transaction);

///  Trait for specifying a mask to select one column value.
pub trait ColumnSelectorOne {
    /// First desired column value
    type T: Decompress;
    /// Mask to obtain desired values, should correspond to the order of columns in a snapshot.
    const MASK: usize;
}

///  Trait for specifying a mask to select two column values.
pub trait ColumnSelectorTwo {
    /// First desired column value
    type T: Decompress;
    /// Second desired column value
    type J: Decompress;
    /// Mask to obtain desired values, should correspond to the order of columns in a snapshot.
    const MASK: usize;
}

///  Trait for specifying a mask to select three column values.
pub trait ColumnSelectorThree {
    /// First desired column value
    type T: Decompress;
    /// Second cdesired olumn value
    type J: Decompress;
    /// Third desired column value
    type K: Decompress;
    /// Mask to obtain desired values, should correspond to the order of columns in a snapshot.
    const MASK: usize;
}

#[macro_export]
/// Add mask to select `N` column values from a specific snapshot segment row.
macro_rules! add_snapshot_mask {
    ($mask_struct:tt, $type1:ty, $mask:expr) => {
        impl ColumnSelectorOne for $mask_struct<$type1> {
            type T = $type1;
            const MASK: usize = $mask;
        }
    };
    ($mask_struct:tt, $type1:ty, $type2:ty, $mask:expr) => {
        impl ColumnSelectorTwo for $mask_struct<$type1, $type2> {
            type T = $type1;
            type J = $type2;
            const MASK: usize = $mask;
        }
    };
    ($mask_struct:tt, $type1:ty, $type2:ty, $type3:ty, $mask:expr) => {
        impl ColumnSelectorTwo for $mask_struct<$type1, $type2, $type3> {
            type T = $type1;
            type J = $type2;
            type K = $type3;
            const MASK: usize = $mask;
        }
    };
}
