#[doc(hidden)]
#[macro_export]
macro_rules! metadata_var {
    ($target:expr, $level:expr) => {{
        static METADATA: $crate::Metadata<'static> = $crate::Metadata::new(
            $target,
            $level,
            ::core::option::Option::Some(::core::module_path!()),
        );
        &METADATA
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! count {
    () => {
        0usize
    };
    ($head:tt $($tail:tt)*) => {
        1usize + $crate::count!($($tail)*)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! key_var {
    ($name: literal) => {{
        static METRIC_KEY: $crate::Key = $crate::Key::from_static_name($name);
        &METRIC_KEY
    }};
    ($name:expr) => {
        $crate::Key::from_name($name)
    };
    ($name:literal, $($label_key:literal => $label_value:literal),*) => {{
        static LABELS: [$crate::Label; $crate::count!($($label_key)*)] = [
            $($crate::Label::from_static_parts($label_key, $label_value)),*
        ];
        static METRIC_KEY: $crate::Key = $crate::Key::from_static_parts($name, &LABELS);
        &METRIC_KEY
    }};
    ($name:expr, $($label_key:literal => $label_value:literal),*) => {{
        static LABELS: [$crate::Label; $crate::count!($($label_key)*)] = [
            $($crate::Label::from_static_parts($label_key, $label_value)),*
        ];
        $crate::Key::from_static_labels($name, &LABELS)
    }};
    ($name:expr, $($label_key:expr => $label_value:expr),*) => {{
        let labels = ::std::vec![
            $($crate::Label::new($label_key, $label_value)),*
        ];
        $crate::Key::from_parts($name, labels)
    }};
    ($name:expr, $labels:expr) => {
        $crate::Key::from_parts($name, $labels)
    }
}

#[doc(hidden)]
#[macro_export]
///Internal macro to register metric description when provided by metric creation macro
macro_rules! __describe_metric {
    // Do nothing if metric description is not set
    ($method:ident, __internal_metric_description_none__, __internal_metric_unit_none__, $($rest:tt)*) => {{}};
    // Show compilation error if `unit` only specified
    ($method:ident, __internal_metric_description_none__, $unit:expr, $($rest:tt)*) => {{
        compile_error!("'unit:' requires to specify parameter 'description:'");
    }};
    // Found description only
    ($method:ident, $description:expr, __internal_metric_unit_none__, $name:expr) => {{
        $crate::with_recorder(|recorder| {
            recorder.$method(
                ::core::convert::Into::into($name),
                ::core::option::Option::None,
                ::core::convert::Into::into($description),
            );
        });
    }};
    // Found description + unit
    ($method:ident, $description:expr, $unit:expr, $name:expr) => {{
        $crate::with_recorder(|recorder| {
            recorder.$method(
                ::core::convert::Into::into($name),
                ::core::option::Option::Some($unit),
                ::core::convert::Into::into($description),
            );
        });
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __register_metric {
    // `target:` — replace the accumulator's `target` slot.
    (
        $describe:ident,
        $register:ident,
        description = $description:tt,
        unit = $unit:tt,
        target = $_old:expr,
        level = $level:expr;
        target: $target:expr,
        $($rest:tt)*
    ) => {
        $crate::__register_metric!(
            $describe,
            $register,
            description = $description,
            unit = $unit,
            target = $target,
            level = $level;
            $($rest)*
        )
    };
    // `level:` — replace the accumulator's `level` slot.
    (
        $describe:ident,
        $register:ident,
        description = $description:tt,
        unit = $unit:tt,
        target = $target:expr,
        level = $_old:expr;
        level: $level:expr,
        $($rest:tt)*
    ) => {
        $crate::__register_metric!(
            $describe,
            $register,
            description = $description,
            unit = $unit,
            target = $target,
            level = $level;
            $($rest)*
        )
    };
    // `description:` — replace the accumulator's `description` slot.
    (
        $describe:ident,
        $register:ident,
        description = $old:tt,
        unit = $unit:tt,
        target = $target:expr,
        level = $level:expr;
        description: $description:expr,
        $($rest:tt)*
    ) => {
        $crate::__register_metric!(
            $describe,
            $register,
            description = $description,
            unit = $unit,
            target = $target,
            level = $level;
            $($rest)*
        )
    };
    // `unit:` — replace the accumulator's `unit` slot.
    (
        $describe:ident,
        $register:ident,
        description = $description:tt,
        unit = $old:tt,
        target = $target:expr,
        level = $level:expr;
        unit: $unit:expr,
        $($rest:tt)*
    ) => {
        $crate::__register_metric!(
            $describe,
            $register,
            description = $description,
            unit = $unit,
            target = $target,
            level = $level;
            $($rest)*
        )
    };
    // Terminator — emit the registration call.
    (
        $describe:ident,
        $register:ident,
        description = $description:tt,
        unit = $unit:tt,
        target = $target:expr,
        level = $level:expr;
        $name:expr $(, $label_key:expr $(=> $label_value:expr)?)* $(,)?
    ) => {{
        $crate::__describe_metric!($describe, $description, $unit, $name);

        let metric_key = $crate::key_var!($name $(, $label_key $(=> $label_value)?)*);
        let metadata = $crate::metadata_var!($target, $level);

        $crate::with_recorder(|recorder| recorder.$register(&metric_key, metadata))
    }};
}

/// Registers a counter.
///
/// Counters represent a single monotonic value, which means the value can only be incremented, not decremented, and
/// always starts out with an initial value of zero.
///
/// A handle to the counter -- [`Counter`](crate::Counter) -- is returned by this macro and can be held on to in order
/// to amortize the cost of registration.
///
/// # Usage
///
/// `counter!([named_param: value,] <$name,> [$labels,])`
///
/// Only a name is required to initialize a counter.
///
/// Named parameters must always come before the counter name, and the counter name must come before any labels.
///
/// ## Required parameters
///
/// - `$name` - Name of the counter. Must be a string literal or an expression that results in `String` or `&'static str`.
///
/// ## Named Parameters
///
/// The following parameters can be provided in any order relative to other named parameters:
///
/// - `target:` - Module path of the counter. Defaults to `::core::module_path!()`.
/// - `level:` - Verbosity level of the counter. Defaults to `INFO`.
/// - `description:` - Description of the counter. If specified, `$name` will be used twice.
/// - `unit:` - Unit of measurement of the counter. Description must be provided in order to specify units.
///
/// ## Labels
///
/// Labels can be passed as _one_ of following:
///
/// - Arbitrary number of `<key> => <value>` where `key` and `value` can be a string literal or an expression that results in `String` or `&'static str`.
/// - Static reference to collection of **Label**.
/// - Collection/iterator that implements [IntoLabels](trait.IntoLabels.html).
///
/// # Example
/// ```
/// # #![no_implicit_prelude]
/// # use ::std::convert::From;
/// # use ::std::format;
/// # use ::std::string::String;
/// # use metrics::counter;
/// # fn main() {
/// // A basic counter:
/// let counter = counter!("some_metric_name");
/// counter.increment(1);
///
/// // Specifying labels inline, including using constants for either the key or value:
/// let counter = counter!("some_metric_name", "service" => "http");
/// counter.absolute(42);
///
/// const SERVICE_LABEL: &'static str = "service";
/// const SERVICE_HTTP: &'static str = "http";
/// let counter = counter!("some_metric_name", SERVICE_LABEL => SERVICE_HTTP);
/// counter.increment(123);
///
/// // We can also pass labels by giving a vector or slice of key/value pairs.  In this scenario,
/// // a unit or description can still be passed in their respective positions:
/// let dynamic_val = "woo";
/// let labels = [("dynamic_key", format!("{}!", dynamic_val))];
/// let counter = counter!("some_metric_name", &labels);
///
/// // As mentioned in the documentation, metric names also can be owned strings, including ones
/// // generated at the callsite via things like `format!`:
/// let name = String::from("some_owned_metric_name");
/// let counter = counter!(name);
///
/// let counter = counter!(format!("{}_via_format", "name"));
///
/// // Using all of the above, we can customize the counter's description, unit, target, and level:
/// let counter = counter!(
///     description: "super counter",
///     unit: metrics::Unit::Bytes,
///     target: ::core::module_path!(),
///     level: metrics::Level::INFO,
///     "super_counter",
///     "label1" => "value1",
///     "label2" => "value2"
/// );
/// # }
/// ```
#[macro_export]
macro_rules! counter {
    ($($input:tt)*) => {
        $crate::__register_metric!(
            describe_counter,
            register_counter,
            description = __internal_metric_description_none__,
            unit = __internal_metric_unit_none__,
            target = ::core::module_path!(),
            level = $crate::Level::INFO;
            $($input)*
        )
    };
}

/// Registers a gauge.
///
/// Gauges represent a single value that can go up or down over time, and always starts out with an initial value of
/// zero.
///
/// A handle to the gauge -- [`Gauge`](crate::Gauge) -- is returned by this macro and can be held on to in order to
/// amortize the cost of registration.
///
/// # Usage
///
/// `gauge!([named_param: value,] <$name,> [$labels,])`
///
/// Only a name is required to initialize a gauge.
///
/// Named parameters must always come before the gauge name, and the gauge name must come before any labels.
///
/// ## Required parameters
///
/// - `$name` - Name of the gauge. Must be a string literal or an expression that results in `String` or `&'static str`.
///
/// ## Named Parameters
///
/// The following parameters can be provided in any order relative to other named parameters:
///
/// - `target:` - Module path of the gauge. Defaults to `::core::module_path!()`.
/// - `level:` - Verbosity level of the gauge. Defaults to `INFO`.
/// - `description:` - Description of the gauge. If specified, `$name` will be used twice.
/// - `unit:` - Unit of measurement of the gauge. Description must be provided in order to specify units.
///
/// ## Labels
///
/// Labels can be passed as _one_ of following:
///
/// - Arbitrary number of `<key> => <value>` where `key` and `value` can be a string literal or an expression that results in `String` or `&'static str`.
/// - Static reference to collection of **Label**.
/// - Collection/iterator that implements [IntoLabels](trait.IntoLabels.html).
///
/// # Example
/// ```
/// # #![no_implicit_prelude]
/// # use ::std::string::String;
/// # use ::std::format;
/// # use ::std::convert::From;
/// # use metrics::gauge;
/// # fn main() {
/// // A basic gauge:
/// let gauge = gauge!("some_metric_name");
/// gauge.increment(1.0);
///
/// // Specifying labels inline, including using constants for either the key or value:
/// let gauge = gauge!("some_metric_name", "service" => "http");
/// gauge.decrement(42.0);
///
/// const SERVICE_LABEL: &'static str = "service";
/// const SERVICE_HTTP: &'static str = "http";
/// let gauge = gauge!("some_metric_name", SERVICE_LABEL => SERVICE_HTTP);
/// gauge.increment(3.14);
///
/// // We can also pass labels by giving a vector or slice of key/value pairs.  In this scenario,
/// // a unit or description can still be passed in their respective positions:
/// let dynamic_val = "woo";
/// let labels = [("dynamic_key", format!("{}!", dynamic_val))];
/// let gauge = gauge!("some_metric_name", &labels);
/// gauge.set(1337.0);
///
/// // As mentioned in the documentation, metric names also can be owned strings, including ones
/// // generated at the callsite via things like `format!`:
/// let name = String::from("some_owned_metric_name");
/// let gauge = gauge!(name);
///
/// let gauge = gauge!(format!("{}_via_format", "name"));
///
/// // Using all of the above, we can customize the gauge's description, unit, target, and level:
/// let gauge = gauge!(
///     description: "super gauge",
///     unit: metrics::Unit::Bytes,
///     target: ::core::module_path!(),
///     level: metrics::Level::INFO,
///     "super_gauge",
///     "label1" => "value1",
///     "label2" => "value2"
/// );
/// # }
/// ```
#[macro_export]
macro_rules! gauge {
    ($($input:tt)*) => {
        $crate::__register_metric!(
            describe_gauge,
            register_gauge,
            description = __internal_metric_description_none__,
            unit = __internal_metric_unit_none__,
            target = ::core::module_path!(),
            level = $crate::Level::INFO;
            $($input)*
        )
    };
}

/// Registers a histogram.
///
/// Histograms measure the distribution of values for a given set of measurements, and start with no initial values.
///
/// A handle to the histogram -- [`Histogram`](crate::Histogram) -- is returned by this macro and can be held on to in
/// order to amortize the cost of registration.
///
/// # Usage
///
/// `histogram!([named_param: value,] <$name,> [$labels,])`
///
/// Only a name is required to initialize a histogram.
///
/// Named parameters must always come before the histogram name, and the histogram name must come before any labels.
///
/// ## Required parameters
///
/// - `$name` - Name of the histogram. Must be a string literal or an expression that results in `String` or `&'static str`.
///
/// ## Named Parameters
///
/// The following parameters can be provided in any order relative to other named parameters:
///
/// - `target:` - Module path of the histogram. Defaults to `::core::module_path!()`.
/// - `level:` - Verbosity level of the histogram. Defaults to `INFO`.
/// - `description:` - Description of the histogram. If specified, `$name` will be used twice.
/// - `unit:` - Unit of measurement of the histogram. Description must be provided in order to specify units.
///
/// ## Labels
///
/// Labels can be passed as _one_ of following:
///
/// - Arbitrary number of `<key> => <value>` where `key` and `value` can be a string literal or an expression that results in `String` or `&'static str`.
/// - Static reference to collection of **Label**.
/// - Collection/iterator that implements [IntoLabels](trait.IntoLabels.html).
///
/// # Example
/// ```
/// # #![no_implicit_prelude]
/// # use ::std::string::String;
/// # use ::std::format;
/// # use ::std::convert::From;
/// # use metrics::histogram;
/// # fn main() {
/// // A basic histogram:
/// let histogram = histogram!("some_metric_name");
/// histogram.record(1.0);
///
/// // Specifying labels inline, including using constants for either the key or value:
/// let histogram = histogram!("some_metric_name", "service" => "http");
///
/// const SERVICE_LABEL: &'static str = "service";
/// const SERVICE_HTTP: &'static str = "http";
/// let histogram = histogram!("some_metric_name", SERVICE_LABEL => SERVICE_HTTP);
///
/// // We can also pass labels by giving a vector or slice of key/value pairs.  In this scenario,
/// // a unit or description can still be passed in their respective positions:
/// let dynamic_val = "woo";
/// let labels = [("dynamic_key", format!("{}!", dynamic_val))];
/// let histogram = histogram!("some_metric_name", &labels);
///
/// // As mentioned in the documentation, metric names also can be owned strings, including ones
/// // generated at the callsite via things like `format!`:
/// let name = String::from("some_owned_metric_name");
/// let histogram = histogram!(name);
///
/// let histogram = histogram!(format!("{}_via_format", "name"));
///
/// // Using all of the above, we can customize the histogram's description, unit, target, and level:
/// let histogram = histogram!(
///     description: "super histogram",
///     unit: metrics::Unit::Bytes,
///     target: ::core::module_path!(),
///     level: metrics::Level::INFO,
///     "super_histogram",
///     "label1" => "value1",
///     "label2" => "value2"
/// );
/// # }
/// ```
#[macro_export]
macro_rules! histogram {
    ($($input:tt)*) => {
        $crate::__register_metric!(
            describe_histogram,
            register_histogram,
            description = __internal_metric_description_none__,
            unit = __internal_metric_unit_none__,
            target = ::core::module_path!(),
            level = $crate::Level::INFO;
            $($input)*
        )
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! describe {
    ($method:ident, $name:expr, $unit:expr, $description:expr $(,)?) => {{
        $crate::with_recorder(|recorder| {
            recorder.$method(
                ::core::convert::Into::into($name),
                ::core::option::Option::Some($unit),
                ::core::convert::Into::into($description),
            );
        });
    }};
    ($method:ident, $name:expr, $description:expr $(,)?) => {{
        $crate::with_recorder(|recorder| {
            recorder.$method(
                ::core::convert::Into::into($name),
                ::core::option::Option::None,
                ::core::convert::Into::into($description),
            );
        });
    }};
}

/// Describes a counter.
///
/// Counters represent a single monotonic value, which means the value can only be incremented, not decremented, and
/// always starts out with an initial value of zero.
///
/// Counters can be described with a free-form string, and optionally, a unit can be provided to describe the value
/// and/or rate of the measurements. Whether or not the installed recorder does anything with the description, or
/// optional unit, is implementation defined.
///
/// # Example
/// ```
/// # #![no_implicit_prelude]
/// # use ::std::convert::From;
/// # use ::std::format;
/// # use ::std::string::String;
/// # use metrics::describe_counter;
/// # use metrics::Unit;
/// # fn main() {
/// // A basic counter:
/// describe_counter!("some_metric_name", "my favorite counter");
///
/// // Providing a unit for a counter:
/// describe_counter!("some_metric_name", Unit::Bytes, "my favorite counter");
///
/// // As mentioned in the documentation, metric names also can be owned strings, including ones
/// // generated at the callsite via things like `format!`:
/// let name = String::from("some_owned_metric_name");
/// describe_counter!(name, "my favorite counter");
///
/// describe_counter!(format!("{}_via_format", "name"), "my favorite counter");
/// # }
/// ```
#[macro_export]
macro_rules! describe_counter {
    ($name:expr, $unit:expr, $description:expr $(,)?) => {
        $crate::describe!(describe_counter, $name, $unit, $description)
    };
    ($name:expr, $description:expr $(,)?) => {
        $crate::describe!(describe_counter, $name, $description)
    };
}

/// Describes a gauge.
///
/// Gauges represent a single value that can go up or down over time, and always starts out with an
/// initial value of zero.
///
/// Gauges can be described with a free-form string, and optionally, a unit can be provided to describe the value
/// and/or rate of the measurements. Whether or not the installed recorder does anything with the description, or
/// optional unit, is implementation defined.
///
/// # Example
/// ```
/// # #![no_implicit_prelude]
/// # use ::std::convert::From;
/// # use ::std::format;
/// # use ::std::string::String;
/// # use metrics::describe_gauge;
/// # use metrics::Unit;
/// # fn main() {
/// // A basic gauge:
/// describe_gauge!("some_metric_name", "my favorite gauge");
///
/// // Providing a unit for a gauge:
/// describe_gauge!("some_metric_name", Unit::Bytes, "my favorite gauge");
///
/// // As mentioned in the documentation, metric names also can be owned strings, including ones
/// // generated at the callsite via things like `format!`:
/// let name = String::from("some_owned_metric_name");
/// describe_gauge!(name, "my favorite gauge");
///
/// describe_gauge!(format!("{}_via_format", "name"), "my favorite gauge");
/// # }
/// ```
#[macro_export]
macro_rules! describe_gauge {
    ($name:expr, $unit:expr, $description:expr $(,)?) => {
        $crate::describe!(describe_gauge, $name, $unit, $description)
    };
    ($name:expr, $description:expr $(,)?) => {
        $crate::describe!(describe_gauge, $name, $description)
    };
}

/// Describes a histogram.
///
/// Histograms measure the distribution of values for a given set of measurements, and start with no
/// initial values.
///
/// Histograms can be described with a free-form string, and optionally, a unit can be provided to describe the value
/// and/or rate of the measurements. Whether or not the installed recorder does anything with the description, or
/// optional unit, is implementation defined.
///
/// # Example
/// ```
/// # #![no_implicit_prelude]
/// # use ::std::convert::From;
/// # use ::std::format;
/// # use ::std::string::String;
/// # use metrics::describe_histogram;
/// # use metrics::Unit;
/// # fn main() {
/// // A basic histogram:
/// describe_histogram!("some_metric_name", "my favorite histogram");
///
/// // Providing a unit for a histogram:
/// describe_histogram!("some_metric_name", Unit::Bytes, "my favorite histogram");
///
/// // As mentioned in the documentation, metric names also can be owned strings, including ones
/// // generated at the callsite via things like `format!`:
/// let name = String::from("some_owned_metric_name");
/// describe_histogram!(name, "my favorite histogram");
///
/// describe_histogram!(format!("{}_via_format", "name"), "my favorite histogram");
/// # }
/// ```
#[macro_export]
macro_rules! describe_histogram {
    ($name:expr, $unit:expr, $description:expr $(,)?) => {
        $crate::describe!(describe_histogram, $name, $unit, $description)
    };
    ($name:expr, $description:expr $(,)?) => {
        $crate::describe!(describe_histogram, $name, $description)
    };
}
