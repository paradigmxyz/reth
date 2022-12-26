use quote::{quote, ToTokens};
use syn::{
    punctuated::Punctuated, Attribute, Data, DeriveInput, Error, Lit, LitStr, MetaNameValue,
    Result, Token,
};

use crate::{metric::Metric, with_attrs::WithAttrs};

pub(crate) fn derive(node: &DeriveInput) -> Result<proc_macro2::TokenStream> {
    let ty = &node.ident;
    let ident_name = ty.to_string();

    let metrics_attr = parse_metrics_attr(node)?;
    let metric_fields = parse_metric_fields(node)?;

    let default_fields = metric_fields
        .iter()
        .map(|metric| {
            let field_name = &metric.field.ident;
            let register_stmt = metric.register_stmt(&metrics_attr.scope)?;
            Ok(quote! {
                #field_name: #register_stmt,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let describe_stmts = metric_fields
        .iter()
        .map(|metric| metric.describe_stmt(&metrics_attr.scope))
        .collect::<Result<Vec<_>>>()?;

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
            /// Describe all exposed metrics. Internally calls `describe_*` macros from
            /// the metrics crate according to the metric type.
            /// Ref: https://docs.rs/metrics/0.20.1/metrics/index.html#macros
            pub fn describe() {
                #(#describe_stmts;)*
            }
        }
    })
}

pub(crate) struct MetricsAttr {
    scope: LitStr,
}

fn parse_metrics_attr(node: &DeriveInput) -> Result<MetricsAttr> {
    let metrics_attr = parse_single_required_attr(node, "metrics")?;
    let parsed =
        metrics_attr.parse_args_with(Punctuated::<MetaNameValue, Token![,]>::parse_terminated)?;
    let mut parsed_iter = parsed.into_iter();
    if let Some(kv) = parsed_iter.next() {
        if !kv.path.is_ident("scope") || parsed_iter.next().is_some() {
            Err(Error::new_spanned(kv, "Only single `scope = ..` value is supported."))
        } else {
            Ok(MetricsAttr { scope: parse_str_lit(kv.lit)? })
        }
    } else {
        Err(Error::new_spanned(metrics_attr, "`scope = ..` must be set."))
    }
}

fn parse_metric_fields(node: &DeriveInput) -> Result<Vec<Metric<'_>>> {
    let Data::Struct(ref data) = node.data else {
        return Err(Error::new_spanned(node, "Only structs are supported."))
    };

    let mut metrics = Vec::with_capacity(data.fields.len());
    for field in data.fields.iter() {
        let (mut describe, mut rename) = (None, None);
        if let Some(metric_attr) = parse_single_attr(field, "metric")? {
            let parsed = metric_attr
                .parse_args_with(Punctuated::<MetaNameValue, Token![,]>::parse_terminated)?;
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
        }

        let description = match describe {
            Some(lit_str) => lit_str.value(),
            // Parse docs only if `describe` attribute was not provided
            None => match parse_docs_to_string(field)? {
                Some(docs_str) => docs_str,
                None => {
                    return Err(Error::new_spanned(
                        field,
                        "Either doc comment or `describe` must be provided.",
                    ))
                }
            },
        };

        metrics.push(Metric::new(field, description, rename));
    }

    Ok(metrics)
}

fn parse_single_attr<'a, T: WithAttrs + ToTokens>(
    token: &'a T,
    ident: &str,
) -> Result<Option<&'a Attribute>> {
    let mut attr_iter = token.attrs().iter().filter(|a| a.path.is_ident(ident));
    if let Some(attr) = attr_iter.next() {
        if let Some(next_attr) = attr_iter.next() {
            Err(Error::new_spanned(
                next_attr,
                format!("Duplicate `#[{ident}(..)]` attribute provided."),
            ))
        } else {
            Ok(Some(attr))
        }
    } else {
        Ok(None)
    }
}

fn parse_single_required_attr<'a, T: WithAttrs + ToTokens>(
    token: &'a T,
    ident: &str,
) -> Result<&'a Attribute> {
    if let Some(attr) = parse_single_attr(token, ident)? {
        Ok(attr)
    } else {
        Err(Error::new_spanned(token, format!("`#[{ident}(..)]` attribute must be provided.")))
    }
}

fn parse_docs_to_string<T: WithAttrs>(token: &T) -> Result<Option<String>> {
    let mut doc_str = None;
    for attr in token.attrs().iter() {
        let meta = attr.parse_meta()?;
        if let syn::Meta::NameValue(meta) = meta {
            if let syn::Lit::Str(doc) = meta.lit {
                let doc_value = doc.value().trim().to_string();
                doc_str = Some(
                    doc_str
                        .map(|prev_doc_value| format!("{prev_doc_value} {doc_value}"))
                        .unwrap_or(doc_value),
                );
            }
        }
    }
    Ok(doc_str)
}

fn parse_str_lit(lit: Lit) -> Result<LitStr> {
    match lit {
        Lit::Str(lit_str) => Ok(lit_str),
        _ => Err(Error::new_spanned(lit, "Value **must** be a string literal.")),
    }
}
