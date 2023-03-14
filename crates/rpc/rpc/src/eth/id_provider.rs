use jsonrpsee::types::SubscriptionId;
use std::fmt::Write;

/// An [IdProvider](jsonrpsee::core::traits::IdProvider) for ethereum subscription ids.
///
/// Returns new hex-string [QUANTITY](https://ethereum.org/en/developers/docs/apis/json-rpc/#quantities-encoding) ids
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct EthSubscriptionIdProvider;

impl jsonrpsee::core::traits::IdProvider for EthSubscriptionIdProvider {
    fn next_id(&self) -> SubscriptionId<'static> {
        to_quantity(rand::random::<u128>())
    }
}

/// Returns a hex quantity string for the given value
///
/// Strips all leading zeros, `0` is returned as `0x0`
#[inline(always)]
fn to_quantity(val: u128) -> SubscriptionId<'static> {
    let bytes = val.to_be_bytes();
    let b = bytes.as_slice();
    let non_zero = b.iter().take_while(|b| **b == 0).count();
    let b = &b[non_zero..];
    if b.is_empty() {
        return SubscriptionId::Str("0x0".into())
    }

    let mut id = String::with_capacity(2 * b.len() + 2);
    id.push_str("0x");
    let first_byte = b[0];
    write!(id, "{first_byte:x}").unwrap();

    for byte in &b[1..] {
        write!(id, "{byte:02x}").unwrap();
    }
    id.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_primitives::U128;

    #[test]
    fn test_id_provider_quantity() {
        let id = to_quantity(0);
        assert_eq!(id, SubscriptionId::Str("0x0".into()));
        let id = to_quantity(1);
        assert_eq!(id, SubscriptionId::Str("0x1".into()));

        for _ in 0..1000 {
            let val = rand::random::<u128>();
            let id = to_quantity(val);
            match id {
                SubscriptionId::Str(id) => {
                    let from_hex: U128 = id.parse().unwrap();
                    assert_eq!(from_hex, U128::from(val));
                }
                SubscriptionId::Num(_) => {
                    unreachable!()
                }
            }
        }
    }
}
