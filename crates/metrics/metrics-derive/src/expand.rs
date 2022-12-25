use quote::quote;
use syn::{
    parse::ParseStream, punctuated::Punctuated, Data, DeriveInput, Error, Lit, LitStr,
    MetaNameValue, Result, Token,
};

use crate::metric::Metric;

pub(crate) fn derive(node: &DeriveInput) -> Result<proc_macro2::TokenStream> {
    let ty = &node.ident;
    let ident_name = ty.to_string();

    let scope = parse_scope_attr(&node)?;
    let metrics = parse_metric_fields(&node)?;

    let default_fields = metrics
        .iter()
        .map(|metric| {
            let field_name = &metric.field.ident;
            let register_stmt = metric.register_stmt(&scope)?;
            Ok(quote! {
                #field_name: #register_stmt,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let describe_stmts =
        metrics.iter().map(|metric| metric.describe_stmt(&scope)).collect::<Result<Vec<_>>>()?;

    Ok(quote! {
        impl Default for #ty {
            fn default() -> Self {
                Self {
                    #(#default_fields)*
                }
            }
        }

        impl std::fmt::Debug for #ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(#ident_name).finish()
            }
        }

        impl #ty {
            /// Describe all exposed metrics
            pub fn describe() {
                #(#describe_stmts;)*
            }
        }
    })
}

fn parse_metric_fields(node: &DeriveInput) -> Result<Vec<Metric<'_>>> {
    let Data::Struct(ref data) = node.data else {
        return Err(Error::new_spanned(node, "only structs are supported"))
    };

    let mut metrics = vec![];
    for field in data.fields.iter() {
        let metric_attr = {
            let mut metric_iter = field.attrs.iter().filter(|a| a.path.is_ident("metric"));
            match metric_iter.next() {
                Some(attr) => {
                    if metric_iter.next().is_none() {
                        attr
                    } else {
                        return Err(Error::new_spanned(attr, "Duplicate `metric` value provided."))
                    }
                }
                None => {
                    return Err(Error::new_spanned(
                        field,
                        "`#[metric(..)]` attribute must be set on the field",
                    ))
                }
            }
        };

        let (describe, rename) = metric_attr.parse_args_with(|input: ParseStream<'_>| {
            let parsed = Punctuated::<MetaNameValue, Token![,]>::parse_terminated(input)?;
            let (mut describe, mut rename) = (None, None);
            for kv in parsed {
                if kv.path.is_ident("describe") {
                    if describe.is_some() {
                        return Err(Error::new_spanned(kv, "Duplicate `describe` value provided."))
                    }
                    describe = Some(parse_str_lit(kv.lit)?);
                } else if kv.path.is_ident("rename") {
                    if rename.is_some() {
                        return Err(Error::new_spanned(kv, "Duplicate `rename` value provided."))
                    }
                    rename = Some(parse_str_lit(kv.lit)?)
                } else {
                    return Err(Error::new_spanned(kv, "Unsupported attribute entry."))
                }
            }
            let Some(describe) = describe else {
                return Err(Error::new_spanned(field,"`describe` must be provided."))
            };
            Ok((describe, rename))
        })?;
        metrics.push(Metric::new(field, describe, rename));
    }

    Ok(metrics)
}

fn parse_str_lit(lit: Lit) -> Result<LitStr> {
    match lit {
        Lit::Str(lit_str) => Ok(lit_str),
        _ => Err(Error::new_spanned(lit, "Value **must** be a string literal.")),
    }
}

fn parse_scope_attr(node: &DeriveInput) -> Result<LitStr> {
    let mut scope_iter = node.attrs.iter().filter(|a| a.path.is_ident("scope"));

    match scope_iter.next() {
        Some(attr) => {
            if scope_iter.next().is_none() {
                attr.parse_args_with(|input: ParseStream<'_>| input.parse::<LitStr>())
            } else {
                Err(Error::new_spanned(attr, "Duplicate `#[scope(..)]` attribute provided."))
            }
        }
        None => Err(Error::new_spanned(node, "`#[scope(..)]` attribute must be set on the field")),
    }
}
