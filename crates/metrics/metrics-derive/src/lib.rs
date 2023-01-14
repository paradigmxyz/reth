#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! This crate provides [Metrics] derive macro

use proc_macro::TokenStream;
use syn::{parse_macro_input, DeriveInput};

#[allow(unused_extern_crates)]
extern crate proc_macro;

mod expand;
mod metric;
mod with_attrs;

/// The [Metrics] derive macro instruments all of the struct fields and
/// creates a [Default] implementation for the struct registering all of
/// the metrics.
///
/// Additionally, it creates a `describe` method on the struct, which
/// internally calls the describe statements for all metric fields.
///
/// Sample usage:
/// ```
/// use reth_metrics_derive::Metrics;
/// use metrics::{Counter, Gauge, Histogram};
///
/// #[derive(Metrics)]
/// #[metrics(scope = "metrics_custom")]
/// pub struct CustomMetrics {
///     /// A gauge with doc comment description.
///     gauge: Gauge,
///     #[metric(rename = "second_gauge", describe = "A gauge with metric attribute description.")]
///     gauge2: Gauge,
///     /// Some doc comment
///     #[metric(describe = "Metric attribute description will be preferred over doc comment.")]
///     counter: Counter,
///     /// A renamed histogram.
///     #[metric(rename = "histogram")]
///     histo: Histogram
/// }
/// ```
///
/// The example above will be expanded to:
/// ```
/// pub struct CustomMetrics {
///     /// A gauge with doc comment description.
///     gauge: metrics::Gauge,
///     gauge2: metrics::Gauge,
///     /// Some doc comment
///     counter: metrics::Counter,
///     /// A renamed histogram.
///     histo: metrics::Histogram
/// }
///
/// impl Default for CustomMetrics {
///     fn default() -> Self {
///         Self {
///             gauge: metrics::register_gauge!("metrics_custom_gauge"),
///             gauge2: metrics::register_gauge!("metrics_custom_second_gauge"),
///             counter: metrics::register_counter!("metrics_custom_counter"),
///             histo: metrics::register_histogram!("metrics_custom_histogram"),
///         }
///     }
/// }
///
/// impl CustomMetrics {
///     /// Describe all exposed metrics
///     pub fn describe() {
///         metrics::describe_gauge!("metrics_custom_gauge", "A gauge with doc comment description.");
///         metrics::describe_gauge!("metrics_custom_second_gauge", "A gauge with metric attribute description.");
///         metrics::describe_counter!("metrics_custom_counter", "Metric attribute description will be preferred over doc comment.");
///         metrics::describe_histogram!("metrics_custom_histogram", "A renamed histogram.");
///     }
/// }
///
/// impl std::fmt::Debug for CustomMetrics {
///     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
///         f.debug_struct("CustomMetrics").finish()
///     }
/// }
/// ```
///
/// Similarly, you can derive metrics with "dynamic" scope,
/// meaning their scope can be set at the time of instantiation.
/// For example:
/// ```
/// use reth_metrics_derive::Metrics;
///
/// #[derive(Metrics)]
/// #[metrics(dynamic = true)]
/// pub struct DynamicScopeMetrics {
///     /// A gauge with doc comment description.
///     gauge: metrics::Gauge,
/// }
/// ```
///
/// The example with dynamic scope will expand to
/// ```
/// pub struct DynamicScopeMetrics {
///     /// A gauge with doc comment description.
///     gauge: metrics::Gauge,
/// }
///
/// impl DynamicScopeMetrics {
///     pub fn new(scope: &str) -> Self {
///         Self {
///             gauge: metrics::register_gauge!(format!("{}{}{}", scope, "_", "gauge"))
///         }
///     }
///
///     pub fn describe(scope: &str) {
///         metrics::describe_gauge!(format!("{}{}{}", scope, "_", "gauge"), "A gauge with doc comment description.");
///     }
/// }
///
/// impl std::fmt::Debug for DynamicScopeMetrics {
///     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
///         f.debug_struct("DynamicScopeMetrics").finish()
///     }
/// }
/// ```
#[proc_macro_derive(Metrics, attributes(metrics, metric))]
pub fn derive_metrics(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand::derive(&input).unwrap_or_else(|err| err.to_compile_error()).into()
}
