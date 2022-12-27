use quote::quote;
use syn::{Error, Field, LitStr, Result, Type};

use crate::expand::MetricsAttr;

const COUNTER_TY: &str = "Counter";
const HISTOGRAM_TY: &str = "Histogram";
const GAUGE_TY: &str = "Gauge";

pub(crate) struct Metric<'a> {
    pub(crate) field: &'a Field,
    description: String,
    rename: Option<LitStr>,
}

impl<'a> Metric<'a> {
    const DEFAULT_SEPARATOR: &str = "_";

    pub(crate) fn new(field: &'a Field, description: String, rename: Option<LitStr>) -> Self {
        Self { field, description, rename }
    }

    pub(crate) fn metric_name(&self, config: &MetricsAttr) -> String {
        let scope = config.scope.value();
        let metric = match self.rename.as_ref() {
            Some(name) => name.value(),
            None => self.field.ident.as_ref().map(ToString::to_string).unwrap_or_default(),
        };
        match config.separator.as_ref() {
            Some(separator) => format!("{scope}{}{metric}", separator.value()),
            None => format!("{scope}{}{metric}", Metric::DEFAULT_SEPARATOR),
        }
    }

    pub(crate) fn register_stmt(&self, config: &MetricsAttr) -> Result<proc_macro2::TokenStream> {
        let metric_name = self.metric_name(config);

        if let Type::Path(ref path_ty) = self.field.ty {
            if let Some(last) = path_ty.path.segments.last() {
                let registrar = match last.ident.to_string().as_str() {
                    COUNTER_TY => quote! { metrics::register_counter! },
                    HISTOGRAM_TY => quote! { metrics::register_histogram! },
                    GAUGE_TY => quote! { metrics::register_gauge! },
                    _ => return Err(Error::new_spanned(path_ty, "Unsupported metric type")),
                };

                return Ok(quote! { #registrar(#metric_name) })
            }
        }

        Err(Error::new_spanned(&self.field.ty, "Unsupported metric type"))
    }

    pub(crate) fn describe_stmt(&self, config: &MetricsAttr) -> Result<proc_macro2::TokenStream> {
        let metric_name = self.metric_name(config);
        let description = &self.description;

        if let Type::Path(ref path_ty) = self.field.ty {
            if let Some(last) = path_ty.path.segments.last() {
                let descriptor = match last.ident.to_string().as_str() {
                    COUNTER_TY => quote! { metrics::describe_counter! },
                    HISTOGRAM_TY => quote! { metrics::describe_histogram! },
                    GAUGE_TY => quote! { metrics::describe_gauge! },
                    _ => return Err(Error::new_spanned(path_ty, "Unsupported metric type")),
                };

                return Ok(quote! { #descriptor(#metric_name, #description) })
            }
        }

        Err(Error::new_spanned(&self.field.ty, "Unsupported metric type"))
    }
}
