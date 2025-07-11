use super::*;
use quote::quote;
use syn::parse2;

#[test]
fn test_compact_with_generics() {
    let input = quote! {
            #[derive(Debug, PartialEq, Clone)]
            pub struct GenericStruct<T> {
                value: T,
                count: u64,
        }
    };

    // Parse the input
    let DeriveInput { ident, data, attrs, generics, .. } = parse2(input).unwrap();
    let fields = get_fields(&data);

    // Generate the code
    let mut output = quote! {};
    output.extend(generate_flag_struct(&ident, &attrs, &generics, &fields, false));
    output.extend(generate_from_to(&ident, &attrs, &generics, &fields, None));

    // The generated impl should include the generic parameter with Compact bound
    let output_str = output.to_string();
    assert!(output_str.contains("impl < T > reth_codecs :: Compact for GenericStruct < T > where T : reth_codecs :: Compact"));
}

#[test]
fn test_compact_with_multiple_generics() {
    let input = quote! {
            #[derive(Debug, PartialEq, Clone)]
            pub struct MultiGenericStruct<T, U> {
                first: T,
                second: U,
                count: u64,
        }
    };

    // Parse the input
    let DeriveInput { ident, data, attrs, generics, .. } = parse2(input).unwrap();
    let fields = get_fields(&data);

    // Generate the code
    let mut output = quote! {};
    output.extend(generate_flag_struct(&ident, &attrs, &generics, &fields, false));
    output.extend(generate_from_to(&ident, &attrs, &generics, &fields, None));

    // The generated impl should include both generic parameters with Compact bounds
    let output_str = output.to_string();
    assert!(output_str.contains("T : reth_codecs :: Compact"));
    assert!(output_str.contains("U : reth_codecs :: Compact"));
}

#[test]
fn test_bitflag_functions_with_generics() {
    let input = quote! {
        #[derive(Debug, PartialEq, Clone)]
        pub struct GenericStruct<T> {
            value: T,
            count: u64,
        }
    };

    // Parse the input
    let DeriveInput { ident, data, attrs, generics, .. } = parse2(input).unwrap();
    let fields = get_fields(&data);

    // Generate the flag struct
    let output = generate_flag_struct(&ident, &attrs, &generics, &fields, false);

    // The generated impl should include the generic parameter in bitflag functions
    let output_str = output.to_string();
    assert!(output_str.contains("impl < T > GenericStruct < T >"));
    assert!(output_str.contains("pub const fn bitflag_encoded_bytes"));
    assert!(output_str.contains("pub const fn bitflag_unused_bits"));
}
