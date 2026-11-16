//! A lightweight metrics facade.
//!
//! The `metrics` crate provides a single metrics API that abstracts over the actual metrics implementation.  Libraries
//! can use the metrics API provided by this crate, and the consumer of those libraries can choose the metrics
//! implementation that is most suitable for its use case.
//!
//! # Overview
//! `metrics` exposes two main concepts: emitting a metric, and recording it.
//!
//! ## Metric types, or kinds
//! This crate supports three fundamental metric types, or kinds: counters, gauges, and histograms.
//!
//! ### Counters
//! A counter is a cumulative metric that represents a monotonically increasing value which can only be increased or be
//! reset to zero on restart. For example, you might use a counter to represent the number of operations performed, or
//! the number of errors that have occurred.
//!
//! Counters are unsigned 64-bit integers.
//!
//! If you have a value that goes up and down over time, consider using a gauge.
//!
//! ### Gauges
//! A gauge is a metric that can go up and down, arbitrarily, over time.
//!
//! Gauges are typically used for measured, external values, such as temperature, throughput, or things like current
//! memory usage.  Even if the value is monotonically increasing, but there is no way to store the delta in order to
//! properly figure out how much to increment by, then a gauge might be a suitable choice.
//!
//! Gauges support two modes: incremental updates, or absolute updates.  This allows callers to use them for external
//! measurements -- where no delta can be computed -- as well as internal measurements.
//!
//! Gauges are floating-point 64-bit numbers.
//!
//! ### Histograms
//! A histogram stores an arbitrary number of observations of a specific measurement and provides statistical analysis
//! over the observed values.  Typically, measurements such as request latency are recorded with histograms: a specific
//! action that is repeated over and over which can have a varying result each time.
//!
//! Histograms are used to explore the distribution of values, allowing a caller to understand the modalities of the
//! distribution, such as whether or not all values are grouped close together, or spread evenly, or even whether or not
//! there are multiple groupings or clusters.
//!
//! Colloquially, histograms are usually associated with percentiles, although by definition, they specifically deal
//! with bucketed or binned values: how many values fell within 0-10, how many fell within 11-20, and so on and so
//! forth.  Percentiles, commonly associated with "summaries", deal with understanding how much of a distribution falls
//! below or at a particular percentage of that distribution: 50% of requests are faster than 500ms, 99% of requests are
//! faster than 2450ms, and so on and so forth.
//!
//! While we use the term "histogram" in `metrics`, we enforce no particular usage of true histograms or summaries.  The
//! choice of output is based entirely on the exporter being used to ship your metric data out of your application.  For
//! example, if you're using [metrics-exporter-prometheus], Prometheus supports both histograms and summaries, and the
//! exporter can be configured to output our "histogram" data as either.  Other exporters may choose to stick to using
//! summaries, as is traditional, in order to generate percentile data.
//!
//! Histograms take floating-point 64-bit numbers.
//!
//! ## Emission
//!
//! Metrics are emitted by utilizing the emission methods.  There is a macro for registering and returning a handle for
//! each fundamental metric type:
//!
//! - [`counter!`] returns the [`Counter`] handle then
//!     - [`Counter::increment`] increments the counter.
//!     - [`Counter::absolute`] sets the counter.
//! - [`gauge!`] returns the [`Gauge`] handle then
//!     - [`Gauge::increment`] increments the gauge.
//!     - [`Gauge::decrement`] decrements the gauge.
//!     - [`Gauge::set`] sets the gauge.
//! - [`histogram!`] for histograms then
//!     - [`Histogram::record`] records a data point.
//!
//! Additionally, metrics can be described -- setting either the unit of measure or long-form description -- by using
//! the `describe_*` macros:
//!
//! - [`describe_counter!`] for counters
//! - [`describe_gauge!`] for gauges
//! - [`describe_histogram!`] for histograms
//!
//! In order to register or emit a metric, you need a way to record these events, which is where [`Recorder`] comes into
//! play.
//!
//! ## Recording
//!
//! The [`Recorder`] trait defines the interface between the registration/emission macros, and exporters, which is how
//! we refer to concrete implementations of [`Recorder`].  The trait defines what the exporters are doing -- recording
//! -- but ultimately exporters are sending data from your application to somewhere else: whether it be a third-party
//! service or logging via standard out.  It's "exporting" the metric data out of your application.
//!
//! Each metric type is usually reserved for a specific type of use case, whether it be tracking a single value or
//! allowing the summation of multiple values, and the respective macros elaborate more on the usage and invariants
//! provided by each.
//!
//! # Getting Started
//!
//! ## In libraries
//!
//! Libraries need only include the `metrics` crate to emit metrics.  When an executable installs a recorder, all
//! included crates which emitting metrics will now emit their metrics to that record, which allows library authors to
//! seamless emit their own metrics without knowing or caring which exporter implementation is chosen, or even if one is
//! installed.
//!
//! In cases where no global recorder is installed, a "noop" recorder lives in its place, which has an incredibly
//! low overhead: an atomic load and comparison.  Libraries can safely instrument their code without fear of ruining
//! baseline performance.
//!
//! ### Examples
//!
//! ```rust
//! use metrics::{counter, histogram};
//!
//! # use std::time::Instant;
//! # pub fn run_query(_: &str) -> u64 { 42 }
//! pub fn process(query: &str) -> u64 {
//!     let start = Instant::now();
//!     let row_count = run_query(query);
//!     let delta = start.elapsed();
//!
//!     histogram!("process.query_time").record(delta);
//!     counter!("process.query_row_count").increment(row_count);
//!
//!     row_count
//! }
//! # fn main() {}
//! ```
//!
//! ## In executables
//!
//! Executables, which themselves can emit their own metrics, are intended to install a global recorder so that metrics
//! can actually be recorded and exported somewhere.
//!
//! Initialization of the global recorder isn't required for macros to function, but any metrics emitted before a global
//! recorder is installed will not be recorded, so initialization and installation of an exporter should happen as early
//! as possible in the application lifecycle.
//!
//! ### Warning
//!
//! The metrics system may only be initialized once.
//!
//! For most use cases, you'll be using an off-the-shelf exporter implementation that hooks up to an existing metrics
//! collection system, or interacts with the existing systems/processes that you use.
//!
//! Out of the box, some exporter implementations are available for you to use:
//!
//! * [metrics-exporter-tcp] - outputs metrics to clients over TCP
//! * [metrics-exporter-prometheus] - serves a Prometheus scrape endpoint
//!
//! You can also implement your own recorder if a suitable one doesn't already exist.
//!
//! # Development
//!
//! The primary interface with `metrics` is through the [`Recorder`] trait, which is the connection between the
//! user-facing emission macros -- `counter!`, and so on -- and the actual logic for handling those metrics and doing
//! something with them, like logging them to the console or sending them to a remote metrics system.
//!
//! ## Keys
//!
//! All metrics are, in essence, the combination of a metric type and metric identifier, such as a histogram called
//! "response_latency".  You could conceivably have multiple metrics with the same name, so long as they are of
//! different types.
//!
//! As the types are enforced/limited by the [`Recorder`] trait itself, the remaining piece is the identifier, which we
//! handle by using [`Key`]. Keys hold both the metric name, and potentially, labels related to the metric. The metric
//! name and labels are always string values.
//!
//! Internally, `metrics` uses a clone-on-write "smart pointer" for these values to optimize cases where the values are
//! static strings, which can provide significant performance benefits.  These smart pointers can also hold owned
//! `String` values, though, so users can mix and match static strings and owned strings without issue.
//!
//! Two [`Key`] objects can be checked for equality and considered to point to the same metric if they are equal.
//! Equality checks both the name of the key and the labels of a key.  Labels are _not_ sorted prior to checking for
//! equality, but insertion order is maintained, so any [`Key`] constructed from the same set of labels in the same
//! order should be equal.
//!
//! It is an implementation detail if a recorder wishes to do an deeper equality check that ignores the order of labels,
//! but practically speaking, metric emission, and thus labels, should be fixed in ordering in nearly all cases, and so
//! it typically is not a problem.
//!
//! ## Registration
//!
//! Recorders must handle the "registration" of a metric.
//!
//! In practice, registration solves two potential problems: providing metadata for a metric, and creating an entry for
//! a metric even though it has not been emitted yet.
//!
//! Callers may wish to provide a human-readable description of what the metric is, or provide the units the metrics
//! uses.  Additionally, users may wish to register their metrics so that they show up in the output of the installed
//! exporter even if the metrics have yet to be emitted.  This allows callers to ensure the metrics output is stable, or
//! allows them to expose all of the potential metrics a system has to offer, again, even if they have not all yet been
//! emitted.
//!
//! As you can see from the trait, the registration methods treats the metadata as optional, and the macros allow users
//! to mix and match whichever fields they want to provide.
//!
//! When a metric is registered, the expectation is that it will show up in output with a default value, so, for
//! example, a counter should be initialized to zero, a histogram would have no values, and so on.
//!
//! ## Metadata
//!
//! When registering a metric, metadata can be provided to further describe the metric, in particular about where in the
//! system it originates from and how verbose it is. This metadata emulates much of the same metadata as `tracing`, as
//! it is intended to be used in a similar way: to provide the ability to filter metrics in a more granular way.
//!
//! Metadata provides three main pieces of information: the verbosity of the metric (level), the part of the system it
//! originates from (target), and the Rust module it originates from (module path).
//!
//! For example, an application may wish to collect high-cardinality metrics, such as telemetry about a feature,
//! including the customers using it. Tracking customer usage could mean having a tag with many possible values, and
//! submitting these metrics to the configured downstream system could be costly or computationally expensive.
//!
//! By setting these metrics to a verbosity level of DEBUG, these metrics could potentially be filtered out at the
//! recorder level, without having to change the application code or manually decide, at the callsite, whether or not to
//! emit the metric.
//!
//! Metadata is exporter-specific, and may be ignored entirely. See the documentation of the specific exporter being
//! used for more information on how metadata is utilized, if at all.
//!
//! ## Emission
//!
//! Likewise, recorders must handle the emission of metrics as well.
//!
//! Comparatively speaking, emission is not too different from registration: you have access to the same [`Key`] as well
//! as the value being emitted.
//!
//! For recorders which temporarily buffer or hold on to values before exporting, a typical approach would be to utilize
//! atomic variables for the storage.  For counters and gauges, this can be done simply by using types like
//! [`AtomicU64`](std::sync::atomic::AtomicU64).  For histograms, this can be slightly tricky as you must hold on to all
//! of the distinct values.  In our helper crate, [`metrics-util`][metrics-util], we've provided a type called
//! [`AtomicBucket`][AtomicBucket].  For exporters that will want to get all of the current values in a batch, while
//! clearing the bucket so that values aren't processed again, [AtomicBucket] provides a simple interface to do so, as
//! well as optimized performance on both the insertion and read side.
//!
//! Combined together, exporter authors can use [`Handle`][Handle], also from the `metrics-util` crate, which provides a
//! consolidated type for holding metric data.  These types, and many more from the `metrics-util` crate, form the basis
//! of typical exporter behavior and have been exposed to help you quickly build a new exporter.
//!
//! ## Installing recorders
//!
//! Recorders, also referred to as exporters, must be "installed" such that the emission macros can access them. As
//! users of `metrics`, you'll typically see exporters provide methods to install themselves that hide the nitty gritty
//! details.  These methods will usually be aptly named, such as `install`.
//!
//! However, at a low level, this can happen in one of two ways: installing a recorder globally, or temporarily using it
//! locally.
//!
//! ### Global recorder
//!
//! The global recorder is the recorder that the macros use by default. It is stored in a static variable accessible by
//! all portions of the compiled application, including dependencies. This is what allows us to provide the same
//! "initialize once, benefit everywhere" behavior that users are familiar with from other telemetry crates like
//! `tracing` and `log`.
//!
//! Only one global recorder can be installed in the lifetime of the process. If a global recorder has already been
//! installed, it cannot be replaced: this is due to the fact that once installed, the recorder is "leaked" so that a
//! static reference can be obtained to it and used by subsequent calls to the emission macros, and any downstream
//! crates.
//!
//! ### Local recorder
//!
//! In many scenarios, such as in unit tests, you may wish to temporarily set a recorder to influence all calls to the
//! emission macros within a specific section of code, without influencing other areas of the code, or being limited by
//! the constraints of only one global recorder being allowed.
//!
//! [`with_local_recorder`] allows you to do this by changing the recorder used by the emission macros for the duration
//! of a given closure. While in that closure, the given recorder will act as if it was the global recorder for the
//! current thread. Once the closure returns, the true global recorder takes priority again for the current thread.
//!
//! [metrics-exporter-tcp]: https://docs.rs/metrics-exporter-tcp
//! [metrics-exporter-prometheus]: https://docs.rs/metrics-exporter-prometheus
//! [metrics-util]: https://docs.rs/metrics-util
//! [AtomicBucket]: https://docs.rs/metrics-util/0.5.0/metrics_util/struct.AtomicBucket.html
//! [Handle]: https://docs.rs/metrics-util/0.5.0/metrics_util/enum.Handle.html
#![deny(missing_docs)]
#![cfg_attr(docsrs, feature(doc_cfg), deny(rustdoc::broken_intra_doc_links))]

pub mod atomics;

mod common;
mod macros;
pub use self::common::*;

mod cow;

mod handles;
pub use self::handles::*;

mod key;
pub use self::key::*;

mod label;
pub use self::label::*;

mod metadata;
pub use self::metadata::*;

mod recorder;
pub use self::recorder::*;
