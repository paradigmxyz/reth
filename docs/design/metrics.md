## Metrics

### Metrics or traces?

A **metric** is a numeric representation of data measured over intervals of time. Metrics are malleable to statistical transformations such as sampling, aggregation and correlation, which make them suited to report the overall health of a system.

A **trace** is a representation of a series of causally related distributed events that encode information about the end-to-end request flow through a distributed system. Traces are used to identify the amount of work done at each layer in an application while preserving causality.

The main difference between metrics and traces is therefore that metrics are system-centric and traces are request-centric: metrics give you insight into how a particular system is doing, while traces help teams identify the path of requests through various services.

**For most things, you likely want a metric**, except for two scenarios:

- For contributors, traces are a good profiling tool
- For end-users that run complicated infrastructure, traces in the RPC component makes sense

### How to add a metric

To add metrics use the [`metrics`][metrics] crate. 
1. Add the code emitting the metric.
2. Add the metrics description in the crate's metrics describer module, e.g.: [stages metrics describer](https://github.com/paradigmxyz/reth/blob/main/crates/stages/src/stages_metrics_describer.rs).
3. Document the metric in this file.

#### Metric anatomy

There are three types of metrics:

- **Counters**: Represent (ideally) monotonically increasing values, e.g. the number of errors that have occurred, the number of blocks processed, etc.
- **Gauges**: Represent metrics that can go up or down arbitrarily over time. Usually they are used to measure things like resource usage (memory, CPU, ...) and throughput.
- **Histograms**: Used to store an arbitrary number of observations of a specific measurement, and provides statistical analysis over the observed values. A typical use case is latency of some operation (writing to disk, responding to a request, ...).

Each metric is identified by a [`Key`][metrics.Key], which itself is composed of a [`KeyName`][metrics.KeyName] and an arbitrary number of [`Label`][metrics.Label]s.

The `KeyName` represents the actual metric name, and the labels are used to further drill down into the metric.

For example, a metric that represents stage progress would have a key name of `stage.progress` and a `stage` label that can be used to get the progress of individual stages.

There will only ever exist one description per metric `KeyName`; it is not possible to add a description for a label, or a `KeyName`/`Label` group.

#### Creating metrics

The `metrics` crate provides three macros per metric variant: `register_<metric>!`, `<metric>!`, and `describe_<metric>!`. Prefer to use these where possible, since they generate the code necessary to register and update metrics under various conditions.

- The `register_<metric>!` macro simply creates the metric and returns a handle to it (e.g. a `Counter`). These metric structs are thread-safe and cheap to clone.
- The `<metric>!` macro registers the metric if it does not exist, and updates it's value.
- The `describe_<metric>!` macro adds an end-user description for the metric.

How the metrics are exposed to the end-user is determined by the CLI.

### Metric best practices

- Use `.` to namespace metrics
  - The top-level namespace should **NOT** be `reth`[^1]
- Metric names should not contain spaces
- Add a unit to the metric where appropriate
  - Use the Prometheus [base units][prom_base_units]

[^1]: The top-level namespace is added by the CLI using [`metrics_util::layers::PrefixLayer`][metrics_util.PrefixLayer].

### Current metrics
#### StagedSync Headers
- `stages.headers.counter`: Number of headers successfully retrieved
- `stages.headers.timeout_error`: Number of timeout errors while requesting headers
- `stages.headers.validation_errors`: Number of validation errors while requesting headers
- `stages.headers.unexpected_errors`: Number of unexpected errors while requesting headers
- `stages.headers.request_time`: Elapsed time of successful header requests

[metrics]: https://docs.rs/metrics
[metrics.Key]: https://docs.rs/metrics/latest/metrics/struct.Key.html
[metrics.KeyName]: https://docs.rs/metrics/latest/metrics/struct.KeyName.html
[metrics.Label]: https://docs.rs/metrics/latest/metrics/struct.Label.html
[prom_base_units]: https://prometheus.io/docs/practices/naming/#base-units
[metrics_util.PrefixLayer]: https://docs.rs/metrics-util/latest/metrics_util/layers/struct.PrefixLayer.html
