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
        let has_lifetime = has_lifetime(&generics);
        
        // Generate the code
        let mut output = quote! {};
        output.extend(generate_flag_struct(&ident, &attrs, has_lifetime, &fields, false));
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
        let has_lifetime = has_lifetime(&generics);
        
        // Generate the code
        let mut output = quote! {};
        output.extend(generate_flag_struct(&ident, &attrs, has_lifetime, &fields, false));
        output.extend(generate_from_to(&ident, &attrs, &generics, &fields, None));

        // The generated impl should include both generic parameters with Compact bounds
        let output_str = output.to_string();
        assert!(output_str.contains("T : reth_codecs :: Compact"));
        assert!(output_str.contains("U : reth_codecs :: Compact"));
}

