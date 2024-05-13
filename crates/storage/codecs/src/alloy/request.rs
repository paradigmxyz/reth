//! Native Compact codec impl for EIP-7685 requests.

use crate::Compact;
use alloy_consensus::Request as AlloyRequest;
use alloy_eips::{
    eip6110::DepositRequest as AlloyDepositRequest,
    eip7002::WithdrawalRequest as AlloyWithdrawalRequest,
};
use alloy_primitives::{Address, FixedBytes};
use bytes::BufMut;
use reth_codecs_derive::main_codec;

/// Request acts as bridge which simplifies Compact implementation for AlloyRequest.
///
/// Notice: Make sure this struct is 1:1 with `alloy_consensus::Request`
#[main_codec]
#[derive(Debug, Clone, PartialEq, Eq)]
enum Request {
    /// An [EIP-6110] deposit request.
    ///
    /// [EIP-6110]: https://eips.ethereum.org/EIPS/eip-6110
    DepositRequest(DepositRequest),
    /// An [EIP-7002] withdrawal request.
    ///
    /// [EIP-7002]: https://eips.ethereum.org/EIPS/eip-7002
    WithdrawalRequest(WithdrawalRequest),
}

impl Default for Request {
    fn default() -> Self {
        Request::DepositRequest(Default::default())
    }
}

#[main_codec]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct DepositRequest {
    pubkey: FixedBytes<48>,
    withdrawal_credentials: FixedBytes<32>,
    amount: u64,
    signature: FixedBytes<96>,
    index: u64,
}

#[main_codec]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct WithdrawalRequest {
    source_address: Address,
    validator_public_key: FixedBytes<48>,
    amount: u64,
}

impl Compact for AlloyRequest {
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: BufMut + AsMut<[u8]>,
    {
        let request = match self {
            AlloyRequest::DepositRequest(req) => Request::DepositRequest(DepositRequest {
                pubkey: req.pubkey,
                withdrawal_credentials: req.withdrawal_credentials,
                amount: req.amount,
                signature: req.signature,
                index: req.index,
            }),
            AlloyRequest::WithdrawalRequest(req) => Request::WithdrawalRequest(WithdrawalRequest {
                source_address: req.source_address,
                validator_public_key: req.validator_public_key,
                amount: req.amount,
            }),
            _ => todo!(),
        };
        request.to_compact(buf)
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (request, buf) = Request::from_compact(buf, len);
        let request = match request {
            Request::DepositRequest(req) => AlloyRequest::DepositRequest(AlloyDepositRequest {
                pubkey: req.pubkey,
                withdrawal_credentials: req.withdrawal_credentials,
                amount: req.amount,
                signature: req.signature,
                index: req.index,
            }),
            Request::WithdrawalRequest(req) => {
                AlloyRequest::WithdrawalRequest(AlloyWithdrawalRequest {
                    source_address: req.source_address,
                    validator_public_key: req.validator_public_key,
                    amount: req.amount,
                })
            }
        };
        (request, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::proptest;

    proptest! {
        #[test]
        fn roundtrip(request: AlloyRequest) {
            let mut buf = Vec::<u8>::new();
            let len = request.to_compact(&mut buf);
            let (decoded, _) = AlloyRequest::from_compact(&buf, len);
            assert_eq!(request, decoded);
        }
    }
}
