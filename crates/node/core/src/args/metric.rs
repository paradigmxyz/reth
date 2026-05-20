use clap::Parser;
use reth_cli_util::{parse_non_zero_duration_from_secs, parse_socket_address};
use std::{net::SocketAddr, time::Duration};

/// Default push gateway interval in seconds.
const DEFAULT_PUSH_GATEWAY_INTERVAL_SECS: u64 = 5;

/// Metrics configuration.
#[derive(Debug, Clone, Parser)]
pub struct MetricArgs {
    /// Enable Prometheus metrics.
    ///
    /// The metrics will be served at the given interface and port.
    #[arg(long="metrics", alias = "metrics.prometheus", value_name = "PROMETHEUS", value_parser = parse_socket_address, help_heading = "Metrics")]
    pub prometheus: Option<SocketAddr>,

    /// URL for pushing Prometheus metrics to a push gateway.
    ///
    /// If set, the node will periodically push metrics to the specified push gateway URL.
    #[arg(
        long = "metrics.prometheus.push.url",
        value_name = "PUSH_GATEWAY_URL",
        help_heading = "Metrics"
    )]
    pub push_gateway_url: Option<String>,

    /// Interval in seconds for pushing metrics to push gateway.
    ///
    /// Default: 5 seconds
    #[arg(
        long = "metrics.prometheus.push.interval",
        default_value = "5",
        value_parser = parse_non_zero_duration_from_secs,
        value_name = "SECONDS",
        help_heading = "Metrics"
    )]
    pub push_gateway_interval: Duration,
}

impl Default for MetricArgs {
    fn default() -> Self {
        Self {
            prometheus: None,
            push_gateway_url: None,
            push_gateway_interval: Duration::from_secs(DEFAULT_PUSH_GATEWAY_INTERVAL_SECS),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{Args, Parser};

    #[derive(Parser)]
    struct CommandParser<T: Args> {
        #[command(flatten)]
        args: T,
    }

    #[test]
    fn metrics_push_interval_rejects_zero() {
        let result = CommandParser::<MetricArgs>::try_parse_from([
            "reth",
            "--metrics.prometheus.push.interval",
            "0",
        ]);

        assert!(result.is_err());
    }

    #[test]
    fn metrics_push_interval_accepts_positive_duration() {
        let args = CommandParser::<MetricArgs>::parse_from([
            "reth",
            "--metrics.prometheus.push.interval",
            "10",
        ])
        .args;

        assert_eq!(args.push_gateway_interval, Duration::from_secs(10));
    }
}
