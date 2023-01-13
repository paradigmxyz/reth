use proc_macro::{self, TokenStream};
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::DeriveInput;

/// If `compact` or `rlp` is passed to `derive_arbitrary`, this function will generate the
/// corresponding proptest roundtrip tests.
pub fn maybe_generate_tests(args: TokenStream, ast: &DeriveInput) -> TokenStream2 {
    let type_ident = ast.ident.clone();

    let mut roundtrips = vec![];
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg.to_string() == "compact" {
            roundtrips.push(quote! {
                {
                    let mut buf = vec![];

                    let len = field.clone().to_compact(&mut buf);
                    let (decoded, _) = super::#type_ident::from_compact(&buf, len);

                    assert!(field == decoded);
                }
            });
        } else if arg.to_string() == "rlp" {
            roundtrips.push(quote! {
                {
                    let mut buf = vec![];

                    let len = field.clone().encode(&mut buf);
                    let decoded = super::#type_ident::decode(&mut buf.as_slice()).unwrap();

                    assert!(field == decoded);
                }
            });
        }
    }

    let mut compact_tests = TokenStream2::default();
    if !roundtrips.is_empty() {
        let mod_tests = format_ident!("{}Tests", ast.ident);

        compact_tests = quote! {
            #[allow(non_snake_case)]
            #[cfg(test)]
            mod #mod_tests {
                use super::*;

                #[test]
                fn proptest() {
                    proptest::proptest!(|(field: super::#type_ident)| {
                        #(#roundtrips)*
                    });
                }
            }
        }
    }

    compact_tests
}
