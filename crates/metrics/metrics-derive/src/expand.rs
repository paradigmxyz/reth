use once_cell::sync::Lazy;
use quote::{quote, ToTokens};
use regex::Regex;
use syn::{
    punctuated::Punctuated, Attribute, Data, DeriveInput, Error, Lit, LitBool, LitStr,
    MetaNameValue, Result, Token,
};

use crate::{metric::Metric, with_attrs::WithAttrs};

/// Metric name regex according to Prometheus data model
///
/// See <https://prometheus.io/docs/concepts/data_model/#metric-names-and-labels>
static METRIC_NAME_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[a-zA-Z_:.][a-zA-Z0-9_:.]*$").unwrap());

/// Supported metrics separators
const SUPPORTED_SEPARATORS: &[&str] = &[".", "_", ":"];

pub(crate) fn derive(node: &DeriveInput) -> Result<proc_macro2::TokenStream> {
    let ty = &node.ident;
    let vis = &node.vis;
    let ident_name = ty.to_string();

    let metrics_attr = parse_metrics_attr(node)?;
    let metric_fields = parse_metric_fields(node)?;

    let describe_doc = quote! {
        /// Describe all exposed metrics. Internally calls `describe_*` macros from
        /// the metrics crate according to the metric type.
        ///
        /// See <https://docs.rs/metrics/0.20.1/metrics/index.html#macros>
    };
    let register_and_describe = match &metrics_attr.scope {
        MetricsScope::Static(scope) => {
            let (defaults, labeled_defaults, describes): (Vec<_>, Vec<_>, Vec<_>) = metric_fields
                .iter()
                .map(|metric| {
                    let field_name = &metric.field.ident;
                    let metric_name =
                        format!("{}{}{}", scope.value(), metrics_attr.separator(), metric.name());
                    let registrar = metric.register_stmt()?;
                    let describe = metric.describe_stmt()?;
                    let description = &metric.description;
                    Ok((
                        quote! {
                            #field_name: #registrar(#metric_name),
                        },
                        quote! {
                            #field_name: #registrar(#metric_name, labels.clone()),
                        },
                        quote! {
                            #describe(#metric_name, #description);
                        },
                    ))
                })
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .fold((vec![], vec![], vec![]), |mut acc, x| {
                    acc.0.push(x.0);
                    acc.1.push(x.1);
                    acc.2.push(x.2);
                    acc
                });

            quote! {
                impl Default for #ty {
                    fn default() -> Self {
                        #ty::describe();

                        Self {
                            #(#defaults)*
                        }
                    }
                }

                impl #ty {
                    /// Create new instance of metrics with provided labels.
                    #vis fn new_with_labels(labels: impl metrics::IntoLabels + Clone) -> Self {
                        Self {
                            #(#labeled_defaults)*
                        }
                    }

                    #describe_doc
                    #vis fn describe() {
                        #(#describes)*
                    }
                }
            }
        }
        MetricsScope::Dynamic => {
            let (defaults, labeled_defaults, describes): (Vec<_>, Vec<_>, Vec<_>) = metric_fields
                .iter()
                .map(|metric| {
                    let name = metric.name();
                    let separator = metrics_attr.separator();
                    let metric_name = quote! {
                        format!("{}{}{}", scope, #separator, #name)
                    };
                    let field_name = &metric.field.ident;

                    let registrar = metric.register_stmt()?;
                    let describe = metric.describe_stmt()?;
                    let description = &metric.description;

                    Ok((
                        quote! {
                            #field_name: #registrar(#metric_name),
                        },
                        quote! {
                            #field_name: #registrar(#metric_name, labels.clone()),
                        },
                        quote! {
                            #describe(#metric_name, #description);
                        },
                    ))
                })
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .fold((vec![], vec![], vec![]), |mut acc, x| {
                    acc.0.push(x.0);
                    acc.1.push(x.1);
                    acc.2.push(x.2);
                    acc
                });

            quote! {
                impl #ty {
                    /// Create new instance of metrics with provided scope.
                    #vis fn new(scope: &str) -> Self {
                        #ty::describe(scope);

                        Self {
                            #(#defaults)*
                        }
                    }

                    /// Create new instance of metrics with provided labels.
                    #vis fn new_with_labels(scope: &str, labels: impl metrics::IntoLabels + Clone) -> Self {
                        Self {
                            #(#labeled_defaults)*
                        }
                    }

                    #describe_doc
                    #vis fn describe(scope: &str) {
                        #(#describes)*
                    }
                }
            }
        }
    };
    Ok(quote! {
        #register_and_describe

        impl std::fmt::Debug for #ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(#ident_name).finish()
            }
        }
    })
}

pub(crate) struct MetricsAttr {
    pub(crate) scope: MetricsScope,
    pub(crate) separator: Option<LitStr>,
}

impl MetricsAttr {
    const DEFAULT_SEPARATOR: &str = ".";

    fn separator(&self) -> String {
        match &self.separator {
            Some(sep) => sep.value(),
            None => MetricsAttr::DEFAULT_SEPARATOR.to_owned(),
        }
    }
}

pub(crate) enum MetricsScope {
    Static(LitStr),
    Dynamic,
}

fn parse_metrics_attr(node: &DeriveInput) -> Result<MetricsAttr> {
    let metrics_attr = parse_single_required_attr(node, "metrics")?;
    let parsed =
        metrics_attr.parse_args_with(Punctuated::<MetaNameValue, Token![,]>::parse_terminated)?;
    let (mut scope, mut separator, mut dynamic) = (None, None, None);
    for kv in parsed {
        if kv.path.is_ident("scope") {
            if scope.is_some() {
                return Err(Error::new_spanned(kv, "Duplicate `scope` value provided."))
            }
            let scope_lit = parse_str_lit(&kv.lit)?;
            validate_metric_name(&scope_lit)?;
            scope = Some(scope_lit);
        } else if kv.path.is_ident("separator") {
            if separator.is_some() {
                return Err(Error::new_spanned(kv, "Duplicate `separator` value provided."))
            }
            let separator_lit = parse_str_lit(&kv.lit)?;
            if !SUPPORTED_SEPARATORS.contains(&&*separator_lit.value()) {
                return Err(Error::new_spanned(
                    kv,
                    format!(
                        "Unsupported `separator` value. Supported: {}.",
                        SUPPORTED_SEPARATORS
                            .iter()
                            .map(|sep| format!("`{sep}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ))
            }
            separator = Some(separator_lit);
        } else if kv.path.is_ident("dynamic") {
            if dynamic.is_some() {
                return Err(Error::new_spanned(kv, "Duplicate `dynamic` flag provided."))
            }
            dynamic = Some(parse_bool_lit(&kv.lit)?.value);
        } else {
            return Err(Error::new_spanned(kv, "Unsupported attribute entry."))
        }
    }

    let scope = match (scope, dynamic) {
        (Some(scope), None) | (Some(scope), Some(false)) => MetricsScope::Static(scope),
        (None, Some(true)) => MetricsScope::Dynamic,
        (Some(_), Some(_)) => {
            return Err(Error::new_spanned(node, "`scope = ..` conflicts with `dynamic = true`."))
        }
        _ => {
            return Err(Error::new_spanned(
                node,
                "Either `scope = ..` or `dynamic = true` must be set.",
            ))
        }
    };

    Ok(MetricsAttr { scope, separator })
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
                    describe = Some(parse_str_lit(&kv.lit)?);
                } else if kv.path.is_ident("rename") {
                    if rename.is_some() {
                        return Err(Error::new_spanned(kv, "Duplicate `rename` value provided."))
                    }
                    let rename_lit = parse_str_lit(&kv.lit)?;
                    validate_metric_name(&rename_lit)?;
                    rename = Some(rename_lit)
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
                        "Either doc comment or `describe = ..` must be set.",
                    ))
                }
            },
        };

        metrics.push(Metric::new(field, description, rename));
    }

    Ok(metrics)
}

fn validate_metric_name(name: &LitStr) -> Result<()> {
    if METRIC_NAME_RE.is_match(&name.value()) {
        Ok(())
    } else {
        Err(Error::new_spanned(name, format!("Value must match regex {}", METRIC_NAME_RE.as_str())))
    }
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

fn parse_str_lit(lit: &Lit) -> Result<LitStr> {
    match lit {
        Lit::Str(lit_str) => Ok(lit_str.to_owned()),
        _ => Err(Error::new_spanned(lit, "Value **must** be a string literal.")),
    }
}

fn parse_bool_lit(lit: &Lit) -> Result<LitBool> {
    match lit {
        Lit::Bool(lit_bool) => Ok(lit_bool.to_owned()),
        _ => Err(Error::new_spanned(lit, "Value **must** be a string literal.")),
    }
}
