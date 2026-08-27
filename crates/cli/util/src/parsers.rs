use alloy_eips::BlockHashOrNumber;
use alloy_primitives::B256;
use reth_fs_util::FsPathError;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs},
    path::Path,
    str::FromStr,
    time::Duration,
};

/// Helper to parse a [Duration] from seconds
pub fn parse_duration_from_secs(arg: &str) -> eyre::Result<Duration, std::num::ParseIntError> {
    let seconds = arg.parse()?;
    Ok(Duration::from_secs(seconds))
}

/// Helper to parse a [Duration] from seconds if it's a number or milliseconds if the input contains
/// a `ms` suffix:
///  * `5ms` -> 5 milliseconds
///  * `5` -> 5 seconds
///  * `5s` -> 5 seconds
pub fn parse_duration_from_secs_or_ms(
    arg: &str,
) -> eyre::Result<Duration, std::num::ParseIntError> {
    if arg.ends_with("ms") {
        arg.trim_end_matches("ms").parse().map(Duration::from_millis)
    } else if arg.ends_with('s') {
        arg.trim_end_matches('s').parse().map(Duration::from_secs)
    } else {
        arg.parse().map(Duration::from_secs)
    }
}

/// Helper to format a [Duration] to the format that can be parsed by
/// [`parse_duration_from_secs_or_ms`].
pub fn format_duration_as_secs_or_ms(duration: Duration) -> String {
    if duration.as_millis().is_multiple_of(1000) {
        format!("{}", duration.as_secs())
    } else {
        format!("{}ms", duration.as_millis())
    }
}

/// Parse [`BlockHashOrNumber`]
pub fn hash_or_num_value_parser(value: &str) -> eyre::Result<BlockHashOrNumber, eyre::Error> {
    match B256::from_str(value) {
        Ok(hash) => Ok(BlockHashOrNumber::Hash(hash)),
        Err(_) => Ok(BlockHashOrNumber::Number(value.parse()?)),
    }
}

/// Error thrown while parsing a socket address.
#[derive(thiserror::Error, Debug)]
pub enum SocketAddressParsingError {
    /// Failed to convert the string into a socket addr
    #[error("could not parse socket address: {0}")]
    Io(#[from] std::io::Error),
    /// Input must not be empty
    #[error("cannot parse socket address from empty string")]
    Empty,
    /// Failed to parse the address
    #[error("could not parse socket address from {0}")]
    Parse(String),
    /// Failed to parse port
    #[error("could not parse port: {0}")]
    Port(#[from] std::num::ParseIntError),
}

/// Parse a [`SocketAddr`] from a `str`.
///
/// The following formats are checked:
///
/// - If the value can be parsed as a `u16` or starts with `:` it is considered a port, and the
///   hostname is set to `localhost`.
/// - If the value contains `:` it is assumed to be the format `<host>:<port>`
/// - Otherwise it is assumed to be a hostname
///
/// An error is returned if the value is empty.
pub fn parse_socket_address(value: &str) -> eyre::Result<SocketAddr, SocketAddressParsingError> {
    if value.is_empty() {
        return Err(SocketAddressParsingError::Empty)
    }

    if let Some(port) = value.strip_prefix(':').or_else(|| value.strip_prefix("localhost:")) {
        let port: u16 = port.parse()?;
        return Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port))
    }
    if let Ok(port) = value.parse() {
        return Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port))
    }
    value
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| SocketAddressParsingError::Parse(value.to_string()))
}

/// Wrapper around [`reth_fs_util::read_json_file`] which can be used as a clap value parser.
pub fn read_json_from_file<T: serde::de::DeserializeOwned>(path: &str) -> Result<T, FsPathError> {
    reth_fs_util::read_json_file(Path::new(path))
}

/// Parses an ether value from a string.
///
/// The amount in eth like "1.05" will be interpreted in wei (1.05 * 1e18).
/// Supports integer, decimal, and scientific notation.
///
/// # Examples
/// - "1.05" -> 1.05 ETH = 1.05 * 10^18 wei
/// - "2" -> 2 ETH = 2 * 10^18 wei
/// - "1e-3" -> 0.001 ETH = 10^15 wei
pub fn parse_ether_value(value: &str) -> eyre::Result<u128> {
    if value.starts_with('-') {
        eyre::bail!("Ether value cannot be negative");
    }

    let value = value.strip_prefix('+').unwrap_or(value);
    let (mantissa, exponent) = if let Some(exponent_index) = value.find(['e', 'E']) {
        let (mantissa, exponent) = value.split_at(exponent_index);
        let exponent = exponent[1..]
            .parse::<i64>()
            .map_err(|_| eyre::eyre!("Invalid ether value: {value}"))?;
        (mantissa, exponent)
    } else {
        (value, 0)
    };

    let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if integer.is_empty() && fraction.is_empty() ||
        !integer.bytes().chain(fraction.bytes()).all(|digit| digit.is_ascii_digit())
    {
        eyre::bail!("Invalid ether value: {value}");
    }

    let mut digits = String::with_capacity(integer.len() + fraction.len());
    digits.push_str(integer);
    digits.push_str(fraction);

    if digits.bytes().all(|digit| digit == b'0') {
        return Ok(0)
    }

    // Interpret the mantissa as an integer, then account for its decimal places and wei units.
    let scale = 18i128 + i128::from(exponent) - i128::try_from(fraction.len())?;
    if scale < 0 {
        // An ether amount must resolve to a whole number of wei.
        let Ok(discarded_digits) = usize::try_from(-scale) else {
            eyre::bail!("Ether value has sub-wei precision");
        };
        if discarded_digits >= digits.len() {
            eyre::bail!("Ether value has sub-wei precision");
        }

        let retained_digits = digits.len() - discarded_digits;
        if !digits[retained_digits..].bytes().all(|digit| digit == b'0') {
            eyre::bail!("Ether value has sub-wei precision");
        }
        digits.truncate(retained_digits);
    }

    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        return Ok(0)
    }

    let wei =
        digits.parse::<u128>().map_err(|_| eyre::eyre!("Ether value exceeds u128::MAX wei"))?;
    let scale = u32::try_from(scale.max(0))
        .map_err(|_| eyre::eyre!("Ether value exceeds u128::MAX wei"))?;
    let multiplier = 10u128
        .checked_pow(scale)
        .ok_or_else(|| eyre::eyre!("Ether value exceeds u128::MAX wei"))?;

    wei.checked_mul(multiplier).ok_or_else(|| eyre::eyre!("Ether value exceeds u128::MAX wei"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;

    #[test]
    fn parse_socket_addresses() {
        for value in ["localhost:9000", ":9000", "9000"] {
            let socket_addr = parse_socket_address(value)
                .unwrap_or_else(|_| panic!("could not parse socket address: {value}"));

            assert!(socket_addr.ip().is_loopback());
            assert_eq!(socket_addr.port(), 9000);
        }
    }

    #[test]
    fn parse_socket_address_random() {
        let port: u16 = rand::rng().random();

        for value in [format!("localhost:{port}"), format!(":{port}"), port.to_string()] {
            let socket_addr = parse_socket_address(&value)
                .unwrap_or_else(|_| panic!("could not parse socket address: {value}"));

            assert!(socket_addr.ip().is_loopback());
            assert_eq!(socket_addr.port(), port);
        }
    }

    #[test]
    fn parse_ms_or_seconds() {
        let ms = parse_duration_from_secs_or_ms("5ms").unwrap();
        assert_eq!(ms, Duration::from_millis(5));

        let seconds = parse_duration_from_secs_or_ms("5").unwrap();
        assert_eq!(seconds, Duration::from_secs(5));

        let seconds = parse_duration_from_secs_or_ms("5s").unwrap();
        assert_eq!(seconds, Duration::from_secs(5));

        assert!(parse_duration_from_secs_or_ms("5ns").is_err());
    }

    #[test]
    fn parse_ether_values() {
        for (value, expected) in [
            ("0", 0),
            ("0e-999", 0),
            ("1.05", 1_050_000_000_000_000_000),
            ("2", 2_000_000_000_000_000_000),
            ("+1", 1_000_000_000_000_000_000),
            (".5", 500_000_000_000_000_000),
            ("1.", 1_000_000_000_000_000_000),
            ("1e-3", 1_000_000_000_000_000),
            ("1E+3", 1_000_000_000_000_000_000_000),
            ("10e-19", 1),
            ("0.0000000000000000010", 1),
            ("1.000000000000000001", 1_000_000_000_000_000_001),
            ("340282366920938463463.374607431768211455", u128::MAX),
        ] {
            assert_eq!(parse_ether_value(value).unwrap(), expected, "failed to parse {value}");
        }

        for invalid in [
            "",
            ".",
            "-0",
            "-1",
            "abc",
            "NaN",
            "inf",
            "1e",
            "e1",
            "1.2.3",
            "1e-19",
            "0.0000000000000000009",
            "340282366920938463463.374607431768211456",
            "441711766194596082395824375185729628956870974218904739530401550323154944",
        ] {
            assert!(parse_ether_value(invalid).is_err(), "accepted {invalid}");
        }
    }
}
