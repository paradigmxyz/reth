use alloc::string::String;
use core::time::Duration;

/// Represents one Kilogas, or `1_000` gas.
pub const KILOGAS: u64 = 1_000;

/// Represents one Megagas, or `1_000_000` gas.
pub const MEGAGAS: u64 = KILOGAS * 1_000;

/// Represents one Gigagas, or `1_000_000_000` gas.
pub const GIGAGAS: u64 = MEGAGAS * 1_000;

/// Returns a formatted gas throughput log, showing either:
///  * "Kgas/s", or 1,000 gas per second
///  * "Mgas/s", or 1,000,000 gas per second
///  * "Ggas/s", or 1,000,000,000 gas per second
///
/// Depending on the magnitude of the gas throughput.
pub fn format_gas_throughput(gas: u64, execution_duration: Duration) -> String {
    let gas_per_second = gas as f64 / execution_duration.as_secs_f64();
    if gas_per_second < MEGAGAS as f64 {
        format!("{:.2} Kgas/second", gas_per_second / KILOGAS as f64)
    } else if gas_per_second < GIGAGAS as f64 {
        format!("{:.2} Mgas/second", gas_per_second / MEGAGAS as f64)
    } else {
        format!("{:.2} Ggas/second", gas_per_second / GIGAGAS as f64)
    }
}

/// Returns a formatted gas log, showing either:
///  * "Kgas", or 1,000 gas
///  * "Mgas", or 1,000,000 gas
///  * "Ggas", or 1,000,000,000 gas
///
/// Depending on the magnitude of gas.
pub fn format_gas(gas: u64) -> String {
    let gas = gas as f64;
    if gas < MEGAGAS as f64 {
        format!("{:.2} Kgas", gas / KILOGAS as f64)
    } else if gas < GIGAGAS as f64 {
        format!("{:.2} Mgas", gas / MEGAGAS as f64)
    } else {
        format!("{:.2} Ggas", gas / GIGAGAS as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gas_fmt() {
        let gas = 100_000;
        let gas_unit = format_gas(gas);
        assert_eq!(gas_unit, "100.00 Kgas");

        let gas = 100_000_000;
        let gas_unit = format_gas(gas);
        assert_eq!(gas_unit, "100.00 Mgas");

        let gas = 100_000_000_000;
        let gas_unit = format_gas(gas);
        assert_eq!(gas_unit, "100.00 Ggas");
    }

    #[test]
    fn test_gas_throughput_fmt() {
        let duration = Duration::from_secs(1);
        let gas = 100_000;
        let throughput = format_gas_throughput(gas, duration);
        assert_eq!(throughput, "100.00 Kgas/second");

        let gas = 100_000_000;
        let throughput = format_gas_throughput(gas, duration);
        assert_eq!(throughput, "100.00 Mgas/second");

        let gas = 100_000_000_000;
        let throughput = format_gas_throughput(gas, duration);
        assert_eq!(throughput, "100.00 Ggas/second");
    }
}
