use bytes::{Buf, Bytes};
pub use codecs_derive::*;
use revm_interpreter::{B160 as H160, B256 as H256, U256};

/// Trait that implements the `Compact` codec.
///
/// When deriving the trait for custom structs, be aware of certain limitations/recommendations:
/// * Works best with structs that only have native types (eg. u64, H256, U256).
/// * Fixed array types (H256, Address, Bloom) are not compacted.
/// * Max size of `T` in `Option<T>` or `Vec<T>` shouldn't exceed `0xffff`.
/// * Any `bytes::Bytes` field **should be placed last**.
/// * Any other type which is not known to the derive module **should be placed last** in they
///   contain a `bytes::Bytes` field.
///
/// The last two points make it easier to decode the data without saving the length on the
/// `StructFlags`. It will fail compilation if it's not respected. If they're alias to known types,
/// add their definitions to `get_bit_size()` or `known_types` in `generator.rs`.
///
/// Regarding the `specialized_to/from_compact` methods: Mainly used as a workaround for not being
/// able to specialize an impl over certain types like Vec<T>/Option<T> where T is a fixed size
/// array like Vec<H256>.
pub trait Compact {
    /// Takes a buffer which can be written to. *Ideally*, it returns the length written to.
    fn to_compact(self, buf: &mut impl bytes::BufMut) -> usize;
    /// Takes a buffer which can be read from. Returns the object and `buf` with its internal cursor
    /// advanced (eg.`.advance(len)`).
    ///
    /// `len` can either be the `buf` remaining length, or the length of the compacted type.
    ///
    /// It will panic, if `len` is smaller than `buf.len()`.
    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8])
    where
        Self: Sized;

    /// "Optional": If there's no good reason to use it, don't.
    fn specialized_to_compact(self, buf: &mut impl bytes::BufMut) -> usize
    where
        Self: Sized,
    {
        self.to_compact(buf)
    }

    /// "Optional": If there's no good reason to use it, don't.
    fn specialized_from_compact(buf: &[u8], len: usize) -> (Self, &[u8])
    where
        Self: Sized,
    {
        Self::from_compact(buf, len)
    }
}

macro_rules! impl_uint_compact {
    ($($name:tt),+) => {
        $(
            impl Compact for $name {
                fn to_compact(self, buf: &mut impl bytes::BufMut) -> usize {
                    let leading = self.leading_zeros() as usize / 8;
                    buf.put_slice(&self.to_be_bytes()[leading..]);
                    std::mem::size_of::<$name>() - leading
                }

                fn from_compact(mut buf: &[u8], len: usize) -> (Self, &[u8]) {
                    if len > 0 {
                        let mut arr = [0; std::mem::size_of::<$name>()];
                        arr[std::mem::size_of::<$name>() - len..].copy_from_slice(&buf[..len]);

                        buf.advance(len);

                        return ($name::from_be_bytes(arr), buf)
                    }
                    (0, buf)
                }
            }
        )+
    };
}

impl_uint_compact!(u64, u128);

impl<T> Compact for Vec<T>
where
    T: Compact + Default,
{
    /// Returns 0 since we won't include it in the `StructFlags`.
    fn to_compact(self, buf: &mut impl bytes::BufMut) -> usize {
        // TODO: can it be smaller?
        buf.put_u16(self.len() as u16);

        for element in self {
            // TODO: elias fano?
            let mut inner = Vec::with_capacity(32);
            buf.put_u16(element.to_compact(&mut inner) as u16);
            buf.put_slice(&inner);
        }
        0
    }

    fn from_compact(mut buf: &[u8], _: usize) -> (Self, &[u8]) {
        let mut list = vec![];
        let length = buf.get_u16();
        for _ in 0..length {
            #[allow(unused_assignments)]
            let mut element = T::default();

            let len = buf.get_u16();

            (element, _) = T::from_compact(&buf[..(len as usize)], len as usize);
            buf.advance(len as usize);

            list.push(element);
        }

        (list, buf)
    }

    /// To be used by fixed sized types like Vec<H256>.
    fn specialized_to_compact(self, buf: &mut impl bytes::BufMut) -> usize {
        buf.put_u16(self.len() as u16);

        for element in self {
            element.to_compact(buf);
        }
        0
    }

    /// To be used by fixed sized types like Vec<H256>.
    fn specialized_from_compact(mut buf: &[u8], len: usize) -> (Self, &[u8]) {
        let mut list = vec![];
        let length = buf.get_u16();
        for _ in 0..length {
            #[allow(unused_assignments)]
            let mut element = T::default();

            (element, buf) = T::from_compact(buf, len);

            list.push(element);
        }

        (list, buf)
    }
}

impl<T> Compact for Option<T>
where
    T: Compact + Default,
{
    /// Returns 0 for `None` and 1 for `Some(_)`.
    fn to_compact(self, buf: &mut impl bytes::BufMut) -> usize {
        if let Some(element) = self {
            let mut inner = vec![];
            let len = element.to_compact(&mut inner);
            buf.put_u16(len as u16);
            buf.put_slice(&inner);
            return 1
        }
        0
    }

    fn from_compact(mut buf: &[u8], len: usize) -> (Self, &[u8]) {
        if len == 0 {
            return (None, buf)
        }

        let len = buf.get_u16();

        let (element, _) = T::from_compact(&buf[..(len as usize)], len as usize);
        buf.advance(len as usize);

        (Some(element), buf)
    }

    /// To be used by fixed sized types like Option<H256>.
    fn specialized_to_compact(self, buf: &mut impl bytes::BufMut) -> usize {
        if let Some(element) = self {
            element.to_compact(buf);
            return 1
        }
        0
    }

    /// To be used by fixed sized types like Option<H256>.
    fn specialized_from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        if len == 0 {
            return (None, buf)
        }

        let (element, buf) = T::from_compact(buf, len);

        (Some(element), buf)
    }
}

impl Compact for U256 {
    fn to_compact(self, buf: &mut impl bytes::BufMut) -> usize {
        let inner: [u8; 32] = self.to_be_bytes();
        let size = 32 - (self.leading_zeros() / 8);
        buf.put_slice(&inner[32 - size..]);
        size
    }

    fn from_compact(mut buf: &[u8], len: usize) -> (Self, &[u8]) {
        if len > 0 {
            let mut arr = [0; 32];
            arr[(32 - len)..].copy_from_slice(&buf[..len]);
            buf.advance(len);
            return (U256::from_be_bytes(arr), buf)
        }

        (U256::ZERO, buf)
    }
}

impl Compact for Bytes {
    fn to_compact(self, buf: &mut impl bytes::BufMut) -> usize {
        let len = self.len();
        buf.put(self);
        len
    }
    fn from_compact(mut buf: &[u8], len: usize) -> (Self, &[u8]) {
        (buf.copy_to_bytes(len), buf)
    }
}

/// Implements the [`Compact`] trait for fixed size hash types like [`H256`].
#[macro_export]
macro_rules! impl_hash_compact {
    ($($name:tt),+) => {
        $(
            impl Compact for $name {
                fn to_compact(self, buf: &mut impl bytes::BufMut) -> usize {
                    buf.put_slice(&self.0);
                    std::mem::size_of::<$name>()
                }

                fn from_compact(mut buf: &[u8], len: usize) -> (Self,&[u8]) {
                    if len == 0 {
                        return ($name::default(), buf)
                    }

                    let v = $name::from_slice(
                        buf.get(..std::mem::size_of::<$name>()).expect("size not matching"),
                    );
                    buf.advance(std::mem::size_of::<$name>());
                    (v, buf)
                }

                fn specialized_to_compact(self, buf: &mut impl bytes::BufMut) -> usize {
                    self.to_compact(buf)
                }

                fn specialized_from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
                    Self::from_compact(buf, len)
                }
            }
        )+
    };
}

impl_hash_compact!(H256, H160);

impl Compact for bool {
    /// `bool` vars go directly to the `StructFlags` and are not written to the buffer.
    fn to_compact(self, _: &mut impl bytes::BufMut) -> usize {
        self as usize
    }

    /// `bool` expects the real value to come in `len`, and does not advance the cursor.
    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        (len != 0, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use revm_interpreter::B160;

    pub type Address = B160;

    #[test]
    fn compact_bytes() {
        let arr = [1, 2, 3, 4, 5];
        let list = bytes::Bytes::copy_from_slice(&arr);
        let mut buf = vec![];
        assert_eq!(list.clone().to_compact(&mut buf), list.len());

        // Add some noise data.
        buf.push(1);

        assert_eq!(&buf[..arr.len()], &arr);
        assert_eq!(bytes::Bytes::from_compact(&buf, list.len()), (list, vec![1].as_slice()));
    }

    #[test]
    fn compact_address() {
        let mut buf = vec![];
        assert_eq!(Address::zero().to_compact(&mut buf), 20);
        assert_eq!(buf, vec![0; 20]);

        // Add some noise data.
        buf.push(1);

        // Address shouldn't care about the len passed, since it's not actually compacted.
        assert_eq!(Address::from_compact(&buf, 1000), (Address::zero(), vec![1u8].as_slice()));
    }

    #[test]
    fn compact_h256() {
        let mut buf = vec![];
        assert_eq!(H256::zero().to_compact(&mut buf), 32);
        assert_eq!(buf, vec![0; 32]);

        // Add some noise data.
        buf.push(1);

        // H256 shouldn't care about the len passed, since it's not actually compacted.
        assert_eq!(H256::from_compact(&buf, 1000), (H256::zero(), vec![1u8].as_slice()));
    }

    #[test]
    fn compact_bool() {
        let _vtrue = true;
        let mut buf = vec![];

        assert_eq!(true.to_compact(&mut buf), 1);
        // Bool vars go directly to the `StructFlags` and not written to the buf.
        assert_eq!(buf.len(), 0);

        assert_eq!(false.to_compact(&mut buf), 0);
        assert_eq!(buf.len(), 0);

        let buf = vec![100u8];

        // Bool expects the real value to come in `len`, and does not advance the cursor.
        assert_eq!(bool::from_compact(&buf, 1), (true, buf.as_slice()));
        assert_eq!(bool::from_compact(&buf, 0), (false, buf.as_slice()));
    }

    #[test]
    fn compact_option() {
        let opt = Some(H256::zero());
        let mut buf = vec![];

        assert_eq!(None::<H256>.to_compact(&mut buf), 0);
        assert_eq!(opt.to_compact(&mut buf), 1);
        assert_eq!(buf.len(), 34);

        assert_eq!(Option::<H256>::from_compact(&buf, 1), (opt, vec![].as_slice()));

        // If `None`, it returns the slice at the same cursor position.
        assert_eq!(Option::<H256>::from_compact(&buf, 0), (None, buf.as_slice()));

        let mut buf = vec![];
        assert_eq!(opt.specialized_to_compact(&mut buf), 1);
        assert_eq!(buf.len(), 32);
        assert_eq!(Option::<H256>::specialized_from_compact(&buf, 1), (opt, vec![].as_slice()));
    }

    #[test]
    fn compact_vec() {
        let list = vec![H256::zero(), H256::zero()];
        let mut buf = vec![];

        // Vec doesn't return a total length
        assert_eq!(list.clone().to_compact(&mut buf), 0);

        // Add some noise data in the end that should be returned by `from_compact`.
        buf.extend([1u8, 2]);

        let mut remaining_buf = buf.as_slice();
        remaining_buf.advance(2 + 2 + 32 + 2 + 32);

        assert_eq!(Vec::<H256>::from_compact(&buf, 0), (list, remaining_buf));
        assert_eq!(remaining_buf, &[1u8, 2]);
    }

    #[test]
    fn compact_u256() {
        let mut buf = vec![];

        assert_eq!(U256::ZERO.to_compact(&mut buf), 0);
        assert!(buf.is_empty());
        assert_eq!(U256::from_compact(&buf, 0), (U256::ZERO, vec![].as_slice()));

        assert_eq!(U256::from(2).to_compact(&mut buf), 1);
        assert_eq!(buf, vec![2u8]);
        assert_eq!(U256::from_compact(&buf, 1), (U256::from(2), vec![].as_slice()));
    }

    #[test]
    fn compact_u64() {
        let mut buf = vec![];

        assert_eq!(0u64.to_compact(&mut buf), 0);
        assert!(buf.is_empty());
        assert_eq!(u64::from_compact(&buf, 0), (0u64, vec![].as_slice()));

        assert_eq!(2u64.to_compact(&mut buf), 1);
        assert_eq!(buf, vec![2u8]);
        assert_eq!(u64::from_compact(&buf, 1), (2u64, vec![].as_slice()));

        let mut buf = vec![];

        assert_eq!(0xffffffffffffffffu64.to_compact(&mut buf), 8);
        assert_eq!(&buf, &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
        assert_eq!(u64::from_compact(&buf, 8), (0xffffffffffffffffu64, vec![].as_slice()));
    }

    #[main_codec]
    #[derive(Debug, PartialEq, Clone)]
    pub struct TestStruct {
        f_u64: u64,
        f_u256: U256,
        f_bool_t: bool,
        f_bool_f: bool,
        f_option_none: Option<H256>,
        f_option_some: Option<H256>,
        f_option_some_u64: Option<u64>,
        f_vec_empty: Vec<H160>,
        f_vec_some: Vec<H160>,
    }

    impl Default for TestStruct {
        fn default() -> Self {
            TestStruct {
                f_u64: 1u64,                                  // 4 bits | 1 byte
                f_u256: U256::from(1u64),                     // 6 bits | 1 byte
                f_bool_f: false,                              // 1 bit  | 0 bytes
                f_bool_t: true,                               // 1 bit  | 0 bytes
                f_option_none: None,                          // 1 bit  | 0 bytes
                f_option_some: Some(H256::zero()),            // 1 bit  | 32 bytes
                f_option_some_u64: Some(0xffffu64),           // 1 bit  | 2 + 2 bytes
                f_vec_empty: vec![],                          // 0 bits | 2 bytes
                f_vec_some: vec![H160::zero(), H160::zero()], // 0 bits | 2 + 20*2 bytes
            }
        }
    }

    #[test]
    fn compact_test_struct() {
        let test = TestStruct::default();
        let mut buf = vec![];
        assert_eq!(
            test.to_compact(&mut buf),
            2 + // TestStructFlags
            1 +
            1 +
            // 0 + 0 + 0 +
            32 +
            2 + 2 +
            2 +
            2 + 20 * 2
        );

        assert_eq!(
            TestStruct::from_compact(&buf, buf.len()),
            (TestStruct::default(), vec![].as_slice())
        );
    }

    #[main_codec]
    #[derive(Debug, PartialEq, Clone, Default)]
    pub enum TestEnum {
        #[default]
        Var0,
        Var1(TestStruct),
        Var2(u64),
    }

    #[cfg(test)]
    #[allow(dead_code)]
    #[test_fuzz::test_fuzz]
    fn compact_test_enum_all_variants(var0: TestEnum, var1: TestEnum, var2: TestEnum) {
        let mut buf = vec![];
        var0.clone().to_compact(&mut buf);
        assert_eq!(TestEnum::from_compact(&buf, buf.len()).0, var0);

        let mut buf = vec![];
        var1.clone().to_compact(&mut buf);
        assert_eq!(TestEnum::from_compact(&buf, buf.len()).0, var1);

        let mut buf = vec![];
        var2.clone().to_compact(&mut buf);
        assert_eq!(TestEnum::from_compact(&buf, buf.len()).0, var2);
    }

    #[test]
    fn compact_test_enum() {
        let var0 = TestEnum::Var0;
        let var1 = TestEnum::Var1(TestStruct::default());
        let var2 = TestEnum::Var2(1u64);

        compact_test_enum_all_variants(var0, var1, var2);
    }
}
