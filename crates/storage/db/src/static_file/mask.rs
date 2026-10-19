use reth_db_api::table::Decompress;

///  Trait for specifying a mask to select one column value.
pub trait ColumnSelectorOne {
    /// First desired column value
    type FIRST: Decompress;
    /// Mask to obtain desired values, should correspond to the order of columns in a `static_file`.
    const MASK: usize;
}

///  Trait for specifying a mask to select two column values.
pub trait ColumnSelectorTwo {
    /// First desired column value
    type FIRST: Decompress;
    /// Second desired column value
    type SECOND: Decompress;
    /// Mask to obtain desired values, should correspond to the order of columns in a `static_file`.
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
    /// Mask to obtain desired values, should correspond to the order of columns in a `static_file`.
    const MASK: usize;
}

#[macro_export]
/// Add mask to select `N` column values from a specific static file segment row.
macro_rules! add_static_file_mask {
    ($(#[$attr:meta])* $mask_struct:ident $(<$generic:ident>)?, $type1:ty, $mask:expr) => {
        $(#[$attr])*
        #[derive(Debug)]
        pub struct $mask_struct$(<$generic>)?$((std::marker::PhantomData<$generic>))?;

        impl$(<$generic>)? ColumnSelectorOne for $mask_struct$(<$generic>)?
        where
            $type1: Send + Sync + std::fmt::Debug + reth_db_api::table::Decompress,
        {
            type FIRST = $type1;
            const MASK: usize = $mask;
        }
    };
    ($(#[$attr:meta])* $mask_struct:ident $(<$generic:ident>)?, $type1:ty, $type2:ty, $mask:expr) => {
        $(#[$attr])*
        #[derive(Debug)]
        pub struct $mask_struct$(<$generic>)?$((std::marker::PhantomData<$generic>))?;

        impl$(<$generic>)? ColumnSelectorTwo for $mask_struct$(<$generic>)?
        where
            $type1: Send + Sync + std::fmt::Debug + reth_db_api::table::Decompress,
            $type2: Send + Sync + std::fmt::Debug + reth_db_api::table::Decompress,
        {
            type FIRST = $type1;
            type SECOND = $type2;
            const MASK: usize = $mask;
        }
    };
    ($(#[$attr:meta])* $mask_struct:ident $(<$generic:ident>)?, $type1:ty, $type2:ty, $type3:ty, $mask:expr) => {
        $(#[$attr])*
        #[derive(Debug)]
        pub struct $mask_struct$(<$generic>)?$((std::marker::PhantomData<$generic>))?;

        impl$(<$generic>)? ColumnSelectorThree for $mask_struct$(<$generic>)?
        where
            $type1: Send + Sync + std::fmt::Debug + reth_db_api::table::Decompress,
            $type2: Send + Sync + std::fmt::Debug + reth_db_api::table::Decompress,
            $type3: Send + Sync + std::fmt::Debug + reth_db_api::table::Decompress,
        {
            type FIRST = $type1;
            type SECOND = $type2;
            type THIRD = $type3;
            const MASK: usize = $mask;
        }
    };
}
