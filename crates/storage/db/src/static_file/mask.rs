use crate::table::Decompress;

/// Generic Mask helper struct for selecting specific column values to read and decompress.
///
/// #### Explanation:
///
/// A `NippyJar` static file row can contain multiple column values. To specify the column values
/// to be read, a mask is utilized.
///
/// For example, a static file with three columns, if the first and last columns are queried, the
/// mask `0b101` would be passed. To select only the second column, the mask `0b010` would be used.
///
/// Since each static file has its own column distribution, different wrapper types are necessary.
/// For instance, `B256` might be the third column in the `Header` segment, while being the second
/// column in another segment. Hence, `Mask<B256>` would only be applicable to one of these
/// scenarios.
///
/// Alongside, the column selector traits (eg. [`ColumnSelectorOne`]) this provides a structured way
/// to tie the types to be decoded to the mask necessary to query them.
#[derive(Debug)]
pub struct Mask<FIRST, SECOND = (), THIRD = ()>(std::marker::PhantomData<(FIRST, SECOND, THIRD)>);

macro_rules! add_segments {
    ($($segment:tt),+) => {
        paste::paste! {
            $(
                #[doc = concat!("Mask for ", stringify!($segment), " static file segment. See [`Mask`] for more.")]
                #[derive(Debug)]
                pub struct [<$segment Mask>]<FIRST, SECOND = (), THIRD = ()>(Mask<FIRST, SECOND, THIRD>);
            )+
        }
    };
}
add_segments!(Header, Receipt, Transaction);

///  Trait for specifying a mask to select one column value.
pub trait ColumnSelectorOne {
    /// First desired column value
    type FIRST: Decompress;
    /// Mask to obtain desired values, should correspond to the order of columns in a static_file.
    const MASK: usize;
}

///  Trait for specifying a mask to select two column values.
pub trait ColumnSelectorTwo {
    /// First desired column value
    type FIRST: Decompress;
    /// Second desired column value
    type SECOND: Decompress;
    /// Mask to obtain desired values, should correspond to the order of columns in a static_file.
    const MASK: usize;
}

///  Trait for specifying a mask to select three column values.
pub trait ColumnSelectorThree {
    /// First desired column value
    type FIRST: Decompress;
    /// Second desired column value
    type SECOND: Decompress;
    /// Third desired column value
    type THIRD: Decompress;
    /// Mask to obtain desired values, should correspond to the order of columns in a static_file.
    const MASK: usize;
}

#[macro_export]
/// Add mask to select `N` column values from a specific static file segment row.
macro_rules! add_static_file_mask {
    ($mask_struct:tt, $type1:ty, $mask:expr) => {
        impl ColumnSelectorOne for $mask_struct<$type1> {
            type FIRST = $type1;
            const MASK: usize = $mask;
        }
    };
    ($mask_struct:tt, $type1:ty, $type2:ty, $mask:expr) => {
        impl ColumnSelectorTwo for $mask_struct<$type1, $type2> {
            type FIRST = $type1;
            type SECOND = $type2;
            const MASK: usize = $mask;
        }
    };
    ($mask_struct:tt, $type1:ty, $type2:ty, $type3:ty, $mask:expr) => {
        impl ColumnSelectorThree for $mask_struct<$type1, $type2, $type3> {
            type FIRST = $type1;
            type SECOND = $type2;
            type THIRD = $type3;
            const MASK: usize = $mask;
        }
    };
}
