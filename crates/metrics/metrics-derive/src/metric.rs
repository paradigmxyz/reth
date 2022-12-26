use quote::quote;
use syn::{Error, Field, LitStr, Result, Type};

const COUNTER_TY: &str = "Counter";
const HISTOGRAM_TY: &str = "Histogram";
const GAUGE_TY: &str = "Gauge";

pub(crate) struct Metric<'a> {
    pub(crate) field: &'a Field,
    description: String,
    rename: Option<LitStr>,
}

impl<'a> Metric<'a> {
    pub(crate) fn new(field: &'a Field, description: String, rename: Option<LitStr>) -> Self {
        Self { field, description, rename }
    }

    pub(crate) fn metric_name(&self, scope: &LitStr) -> String {
        let scope = scope.value();
        let metric = match self.rename.as_ref() {
            Some(name) => name.value(),
            None => self.field.ident.as_ref().map(ToString::to_string).unwrap_or_default(),
        };
        format!("{scope}.{metric}")
    }

    pub(crate) fn register_stmt(&self, scope: &LitStr) -> Result<proc_macro2::TokenStream> {
        let metric_name = self.metric_name(scope);

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

    pub(crate) fn describe_stmt(&self, scope: &LitStr) -> Result<proc_macro2::TokenStream> {
        let metric_name = self.metric_name(scope);
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
