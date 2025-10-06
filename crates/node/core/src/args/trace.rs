use clap::Parser;

/// Otlp tracing configuration through CLI args.
#[derive(Debug, Clone, Default, Parser)]
pub struct TraceArgs {
    /// Enable OTLP tracing.
    ///
    /// The otlp tracing will be enabled.
    #[arg(
        long = "tracing-otlp",
        global = true,
        alias = "tracing.otlp",
        value_name = "TRACING-OTLP",
        help_heading = "Tracing"
    )]
    pub otlp: bool,
}
