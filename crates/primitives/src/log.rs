use crate::Address;
use crate::H256;
use bytes::Bytes;


/// Ethereum Log
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Log {
    /// Contract that have emited tihs log.
    pub address: Address,
    /// Topics of log, number depends if LOG0 or LOG4 opcode is used.
    pub topics: Vec<H256>,
    /// Arbitrary length data.
    pub data: Bytes,
}
