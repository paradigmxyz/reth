use once_cell::sync::Lazy;
use quote::quote;
use syn::{Error, Field, LitStr, Result};

use crate::loose_path::LooseTypePath;

const COUNTER_TY: Lazy<LooseTypePath> =
    Lazy::new(|| LooseTypePath::parse_from_ty(syn::parse_quote! { metrics::Counter }).unwrap());
const HISTOGRAM_TY: Lazy<LooseTypePath> =
    Lazy::new(|| LooseTypePath::parse_from_ty(syn::parse_quote! { metrics::Histogram }).unwrap());
const GAUGE_TY: Lazy<LooseTypePath> =
    Lazy::new(|| LooseTypePath::parse_from_ty(syn::parse_quote! { metrics::Gauge }).unwrap());

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
            Some(name) => name.token().to_string(),
            None => self.field.ident.as_ref().map(ToString::to_string).unwrap_or_default(),
        };
        format!("{scope}.{metric}")
    }

    pub(crate) fn register_stmt(&self, scope: &LitStr) -> Result<proc_macro2::TokenStream> {
        let metric_name = self.metric_name(scope);
        let ty = LooseTypePath::parse_from_ty(self.field.ty.clone())?;

        let registrar = {
            if ty.eq(&*COUNTER_TY) {
                quote! { metrics::register_counter! }
            } else if ty.eq(&*HISTOGRAM_TY) {
                quote! { metrics::register_histogram! }
            } else if ty.eq(&*GAUGE_TY) {
                quote! { metrics::register_gauge! }
            } else {
                return Err(Error::new_spanned(
                    self.field,
                    format!("Unsupported metric type {:?}", self.field.ty),
                ))
            }
        };

        Ok(quote! { #registrar(#metric_name) })
    }

    pub(crate) fn describe_stmt(&self, scope: &LitStr) -> Result<proc_macro2::TokenStream> {
        let metric_name = self.metric_name(scope);
        let description = &self.description;
        let ty = LooseTypePath::parse_from_ty(self.field.ty.clone())?;

        let descriptor = {
            if ty.eq(&*COUNTER_TY) {
                quote! { metrics::describe_counter! }
            } else if ty.eq(&*HISTOGRAM_TY) {
                quote! { metrics::describe_histogram! }
            } else if ty.eq(&*GAUGE_TY) {
                quote! { metrics::describe_gauge! }
            } else {
                return Err(Error::new_spanned(self.field, "Unsupported metric type"))
            }
        };

        Ok(quote! { #descriptor(#metric_name, #description) })
    }
}
