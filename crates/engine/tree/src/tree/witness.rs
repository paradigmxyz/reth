//! Encoding for the JSON-RPC Engine API witness extension.

use alloy_primitives::Bytes;
use alloy_rlp::{Encodable, Header};
use alloy_rpc_types_debug::ExecutionWitness;

/// Encodes Geth's external witness as `[headers, codes, state, keys]`.
/// Headers are nested RLP lists, whereas codes and state nodes are byte strings.
/// See <https://github.com/ethereum/go-ethereum/blob/7538039f06792da46a91165e7eda98917edcfde2/core/stateless/encoding.go>.
pub(super) fn encode_witness(witness: ExecutionWitness) -> Bytes {
    let mut fields = Vec::new();
    Header { list: true, payload_length: witness.headers.iter().map(|header| header.len()).sum() }
        .encode(&mut fields);
    for header in witness.headers {
        fields.extend_from_slice(&header);
    }
    witness.codes.encode(&mut fields);
    witness.state.encode(&mut fields);
    // Canonical witnesses do not need the debug API's unhashed account/storage keys.
    Vec::<Bytes>::new().encode(&mut fields);
    let mut encoded = Vec::new();
    Header { list: true, payload_length: fields.len() }.encode(&mut encoded);
    encoded.extend_from_slice(&fields);
    encoded.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::hex;

    #[test]
    fn geth_witness_rlp_uses_nested_headers_and_four_fields() {
        let witness = ExecutionWitness {
            headers: vec![Bytes::from_static(&hex!("c101"))],
            codes: vec![Bytes::from_static(b"code")],
            state: vec![Bytes::from_static(b"node")],
            keys: vec![Bytes::from_static(b"debug-only")],
        };
        assert_eq!(encode_witness(witness).as_ref(), &hex!("d0c2c101c584636f6465c5846e6f6465c0"));
    }
}
