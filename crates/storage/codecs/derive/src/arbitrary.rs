use proc_macro::TokenStream;
use proc_macro2::{Ident, TokenStream as TokenStream2};
use quote::{quote, ToTokens};

/// If `compact` or `rlp` is passed to `derive_arbitrary`, this function will generate the
/// corresponding proptest roundtrip tests.
///
/// It accepts an optional integer number for the number of proptest cases. Otherwise, it will set
/// it at 1000.
pub fn maybe_generate_tests(
    args: TokenStream,
    type_ident: &impl ToTokens,
    mod_tests: &Ident,
) -> TokenStream2 {
    // Same as proptest
    let mut default_cases = 256;

    let mut traits = vec![];
    let mut roundtrips = vec![];
    let mut additional_tests = vec![];

    for arg in args {
        if arg.to_string() == "compact" {
            traits.push(quote! { use super::Compact; });
            roundtrips.push(quote! {
                {
                    let mut buf = vec![];
                    let len = field.clone().to_compact(&mut buf);
                    let (decoded, _): (super::#type_ident, _) = Compact::from_compact(&buf, len);
                    assert_eq!(field, decoded, "maybe_generate_tests::compact");
                }
            });
        } else if arg.to_string() == "rlp" {
            traits.push(quote! { use alloy_rlp::{Encodable, Decodable}; });
            roundtrips.push(quote! {
                {
                    let mut buf = vec![];
                    let len = field.encode(&mut buf);
                    let mut b = &mut buf.as_slice();
                    let decoded: super::#type_ident = Decodable::decode(b).unwrap();
                    assert_eq!(field, decoded, "maybe_generate_tests::rlp");
                    // ensure buffer is fully consumed by decode
                    assert!(b.is_empty(), "buffer was not consumed entirely");

                }
            });
            additional_tests.push(quote! {

                #[test]
                fn malformed_rlp_header_check() {
                    use rand::RngCore;

                    // get random instance of type
                    let mut raw = [0u8; 1024];
                    rand::thread_rng().fill_bytes(&mut raw);
                    let mut unstructured = arbitrary::Unstructured::new(&raw[..]);
                    let val: Result<super::#type_ident, _> = arbitrary::Arbitrary::arbitrary(&mut unstructured);
                    if val.is_err() {
                        // this can be flaky sometimes due to not enough data for iterator based types like Vec
                        return
                    }
                    let val = val.unwrap();
                    let mut buf = vec![];
                    let len = val.encode(&mut buf);

                    // malformed rlp-header check
                    let mut decode_buf = &mut buf.as_slice();
                    let mut header = alloy_rlp::Header::decode(decode_buf).expect("failed to decode header");
                    header.payload_length+=1;
                    let mut b = Vec::with_capacity(decode_buf.len());
                    header.encode(&mut b);
                    b.extend_from_slice(decode_buf);
                    let res: Result<super::#type_ident, _> = Decodable::decode(&mut b.as_ref());
                    assert!(res.is_err(), "malformed header was decoded");
                }
            });
        } else if let Ok(num) = arg.to_string().parse() {
            default_cases = num;
        }
    }

    let mut tests = TokenStream2::default();
    if !roundtrips.is_empty() {
        tests = quote! {
            #[allow(non_snake_case)]
            #[cfg(test)]
            mod #mod_tests {
                #(#traits)*
                use proptest_arbitrary_interop::arb;

                #[test]
                fn proptest() {
                    let mut config = proptest::prelude::ProptestConfig::with_cases(#default_cases as u32);

                    proptest::proptest!(config, |(field in arb::<super::#type_ident>())| {
                        #(#roundtrips)*
                    });
                }

                #(#additional_tests)*
            }
        }
    }

    tests
}
