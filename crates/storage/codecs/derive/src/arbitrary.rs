use proc_macro::{self, TokenStream};
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::DeriveInput;

/// If `compact` or `rlp` is passed to `derive_arbitrary`, this function will generate the
/// corresponding proptest roundtrip tests.
///
/// It accepts an optional integer number for the number of proptest cases. Otherwise, it will set
/// it at 1000.
pub fn maybe_generate_tests(args: TokenStream, ast: &DeriveInput) -> TokenStream2 {
    let type_ident = ast.ident.clone();

    // Same as proptest
    let mut default_cases = 256;

    let mut traits = vec![];
    let mut roundtrips = vec![];

    for arg in args {
        if arg.to_string() == "compact" {
            traits.push(quote! { use super::Compact; });
            roundtrips.push(quote! {
                {
                    let mut buf = vec![];

                    let len = field.clone().to_compact(&mut buf);
                    let (decoded, _) = super::#type_ident::from_compact(&buf, len);

                    assert!(field == decoded);
                }
            });
        } else if arg.to_string() == "rlp" {
            traits.push(quote! { use reth_rlp::{Encodable, Decodable}; });
            roundtrips.push(quote! {
                {
                    let mut buf = vec![];

                    let len = field.encode(&mut buf);
                    let mut b = &mut buf.as_slice();
                    let decoded = super::#type_ident::decode(b).unwrap();
                    assert_eq!(field, decoded);
                    // ensure buffer is fully consumed by decode
                    assert!(b.is_empty());
                }
            });
        } else if let Ok(num) = arg.to_string().parse() {
            default_cases = num;
        }
    }

    let mut tests = TokenStream2::default();
    if !roundtrips.is_empty() {
        let mod_tests = format_ident!("{}Tests", ast.ident);

        tests = quote! {
            #[allow(non_snake_case)]
            #[cfg(test)]
            mod #mod_tests {
                #(#traits)*

                #[test]
                fn proptest() {
                    let mut config = proptest::prelude::ProptestConfig::with_cases(#default_cases as u32);

                    proptest::proptest!(config, |(field: super::#type_ident)| {
                        #(#roundtrips)*
                    });
                }
            }
        }
    }

    tests
}
