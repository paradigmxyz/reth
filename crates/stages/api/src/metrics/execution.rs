use std::time::Duration;

use reth_primitives_traits::constants::gas_units::{GIGAGAS, KILOGAS, MEGAGAS};

/// Returns a formatted gas throughput log, showing either:
///  * "Kgas/s", or 1,000 gas per second
///  * "Mgas/s", or 1,000,000 gas per second
///  * "Ggas/s", or 1,000,000,000 gas per second
///
/// Depending on the magnitude of the gas throughput.
pub fn format_gas_throughput(gas: u64, execution_duration: Duration) -> String {
    let gas_per_second = gas as f64 / execution_duration.as_secs_f64();
    if gas_per_second < MEGAGAS as f64 {
        format!("{:.} Kgas/second", gas_per_second / KILOGAS as f64)
    } else if gas_per_second < GIGAGAS as f64 {
        format!("{:.} Mgas/second", gas_per_second / MEGAGAS as f64)
    } else {
        format!("{:.} Ggas/second", gas_per_second / GIGAGAS as f64)
    }
}
