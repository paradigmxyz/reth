#[macro_export]
macro_rules! table {
    ($name:ident => $key:ty => $value:ty => $seek:ty) => {
        #[derive(Clone, Copy, Debug, Default)]
        pub struct $name;

        impl $crate::kv::table::Table for $name {
            type Key = $key;
            type Value = $value;
            type SeekKey = $seek;

            fn db_name(&self) -> &'static str  {
               stringify!($name)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", stringify!($name))
            }
        }
    };
    ($name:ident => $key:ty => $value:ty) => {
        table!($name => $key => $value => $key);
    };
}

table!(Account => Vec<u8> => Vec<u8> => Vec<u8>);
