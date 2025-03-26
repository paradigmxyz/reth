//! Native Compact codec impl for primitive alloy log types.

use crate::Compact;
use alloc::vec::Vec;
use alloy_primitives::{Address, Bytes, Log, LogData};
use bytes::BufMut;

/// Implement `Compact` for `LogData` and `Log`.
impl Compact for LogData {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: BufMut + AsMut<[u8]>,
    {
        let mut buffer = Vec::new();

        self.topics().specialized_to_compact(&mut buffer);
        self.data.to_compact(&mut buffer);
        buf.put(&buffer[..]);
        buffer.len()
    }

    fn from_compact(mut buf: &[u8], _: usize) -> (Self, &[u8]) {
        let (topics, new_buf) = Vec::specialized_from_compact(buf, buf.len());
        buf = new_buf;
        let (data, buf) = Bytes::from_compact(buf, buf.len());
        let log_data = Self::new_unchecked(topics, data);
        (log_data, buf)
    }
}

impl Compact for Log {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: BufMut + AsMut<[u8]>,
    {
        let mut buffer = Vec::new();
        self.address.to_compact(&mut buffer);
        self.data.to_compact(&mut buffer);
        buf.put(&buffer[..]);
        buffer.len()
    }

    fn from_compact(mut buf: &[u8], _: usize) -> (Self, &[u8]) {
        let (address, new_buf) = Address::from_compact(buf, buf.len());
        buf = new_buf;
        let (log_data, new_buf) = LogData::from_compact(buf, buf.len());
        buf = new_buf;
        let log = Self { address, data: log_data };
        (log, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::{Compact, Log};
    use alloy_primitives::{Address, Bytes, LogData, B256};
    use proptest::proptest;
    use serde::Deserialize;

    proptest! {
        #[test]
        fn roundtrip(log: Log) {
            let mut buf = Vec::<u8>::new();
            let len = log.to_compact(&mut buf);
            let (decoded, _) = Log::from_compact(&buf, len);
            assert_eq!(log, decoded);
        }
    }

    #[derive(Deserialize)]
    struct CompactLogTestVector {
        topics: Vec<B256>,
        address: Address,
        data: Bytes,
        encoded_bytes: Bytes,
    }

    #[test]
    fn test_compact_log_codec() {
        let test_vectors: Vec<CompactLogTestVector> =
            serde_json::from_str(include_str!("../../testdata/log_compact.json"))
                .expect("Failed to parse test vectors");

        for test_vector in test_vectors {
            let log_data = LogData::new_unchecked(test_vector.topics, test_vector.data);
            let log = Log { address: test_vector.address, data: log_data };

            let mut buf = Vec::<u8>::new();
            let len = log.clone().to_compact(&mut buf);
            assert_eq!(test_vector.encoded_bytes, buf);

            let (decoded, _) = Log::from_compact(&test_vector.encoded_bytes, len);
            assert_eq!(log, decoded);
        }
    }
}
