//! This exposes the process CLI arguments over prometheus.
use metrics::gauge;

/// Registers the process CLI arguments as a prometheus metric.
///
/// This captures the flags passed to the binary via [`std::env::args`] and emits them as a
/// `process_cli_args` gauge set to `1` with an `args` label containing flag names only.
///
/// Values are stripped to avoid leaking secrets (e.g. `--p2p.secret KEY` becomes `--p2p.secret`).
pub fn register_process_metrics() {
    let args: String = std::env::args()
        .filter(|arg| arg.starts_with('-'))
        .map(|arg| {
            // Strip values from `--flag=value` style arguments
            match arg.split_once('=') {
                Some((flag, _)) => flag.to_string(),
                None => arg,
            }
        })
        .collect::<Vec<_>>()
        .join(" ");

    let gauge = gauge!("process_cli_args", "args" => args);
    gauge.set(1);
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_args_stripping() {
        // Simulates what the filter+map does
        let raw = vec![
            "reth",
            "node",
            "--http",
            "--p2p.secret",
            "MYSECRET",
            "--chain=mainnet",
            "--rpc.port",
            "8545",
            "-v",
        ];

        let result: Vec<_> = raw
            .into_iter()
            .filter(|arg| arg.starts_with('-'))
            .map(|arg| match arg.split_once('=') {
                Some((flag, _)) => flag.to_string(),
                None => arg.to_string(),
            })
            .collect();

        assert_eq!(result, vec!["--http", "--p2p.secret", "--chain", "--rpc.port", "-v"]);
    }
}
