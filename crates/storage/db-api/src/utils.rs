#[macro_export]
/// Implements the `Arbitrary` trait for types with fixed array types.
macro_rules! impl_fixed_arbitrary {
    ($(($name:ident, $size:expr)),*) => {
        #[cfg(any(test, feature = "arbitrary"))]
        use arbitrary::{Arbitrary, Unstructured};
        $(
            #[cfg(any(test, feature = "arbitrary"))]
            impl<'a> Arbitrary<'a> for $name {
                fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self, arbitrary::Error> {
                    let mut buffer = vec![0; $size];
                    u.fill_buffer(buffer.as_mut_slice())?;
                    Decode::decode_owned(buffer).map_err(|_| arbitrary::Error::IncorrectFormat)
                }
            }

            #[cfg(any(test, feature = "arbitrary"))]
            impl proptest::prelude::Arbitrary for $name {
                type Parameters = ();
                type Strategy = proptest::strategy::Map<
                    proptest::collection::VecStrategy<<u8 as proptest::arbitrary::Arbitrary>::Strategy>,
                    fn(Vec<u8>) -> Self,
                >;

                fn arbitrary_with(args: Self::Parameters) -> Self::Strategy {
                    use proptest::strategy::Strategy;
                    proptest::collection::vec(proptest::arbitrary::any_with::<u8>(args), $size)
                        .prop_map(move |vec| Decode::decode_owned(vec).unwrap())
                }
            }
        )+
    };
}
