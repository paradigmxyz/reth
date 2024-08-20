use alloy_primitives::Bloom;
pub use alloy_primitives::{Log, LogData};

/// Calculate receipt logs bloom.
pub fn logs_bloom<'a>(logs: impl IntoIterator<Item = &'a Log>) -> Bloom {
    let mut bloom = Bloom::ZERO;
    for log in logs {
        bloom.m3_2048(log.address.as_slice());
        for topic in log.topics() {
            bloom.m3_2048(topic.as_slice());
        }
    }
    bloom
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{Address, Bytes, Log as AlloyLog, B256};
    use alloy_rlp::{RlpDecodable, RlpEncodable};
    use proptest::proptest;
    use proptest_arbitrary_interop::arb;
    use reth_codecs::{add_arbitrary_tests, Compact};
    use serde::{Deserialize, Serialize};

    /// This type is kept for compatibility tests after the codec support was added to
    /// alloy-primitives Log type natively
    #[derive(
        Clone,
        Debug,
        PartialEq,
        Eq,
        RlpDecodable,
        RlpEncodable,
        Default,
        Serialize,
        Deserialize,
        Compact,
    )]
    #[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
    #[add_arbitrary_tests(compact, rlp)]
    struct Log {
        /// Contract that emitted this log.
        address: Address,
        /// Topics of the log. The number of logs depend on what `LOG` opcode is used.
        topics: Vec<B256>,
        /// Arbitrary length data.
        data: Bytes,
    }

    impl From<AlloyLog> for Log {
        fn from(mut log: AlloyLog) -> Self {
            Self {
                address: log.address,
                topics: std::mem::take(log.data.topics_mut_unchecked()),
                data: log.data.data,
            }
        }
    }

    impl From<Log> for AlloyLog {
        fn from(log: Log) -> Self {
            Self::new_unchecked(log.address, log.topics, log.data)
        }
    }

    proptest! {
        #[test]
        fn test_roundtrip_conversion_between_log_and_alloy_log(log in arb::<Log>()) {
            // Convert log to buffer and then create alloy_log from buffer and compare
            let mut compacted_log = Vec::<u8>::new();
            let len = log.to_compact(&mut compacted_log);

            let alloy_log = AlloyLog::from_compact(&compacted_log, len).0;
            assert_eq!(log, alloy_log.into());

            // Create alloy_log from log and then convert it to buffer and compare compacted_alloy_log and compacted_log
            let alloy_log = AlloyLog::new_unchecked(log.address, log.topics, log.data);
            let mut compacted_alloy_log = Vec::<u8>::new();
            let alloy_len = alloy_log.to_compact(&mut compacted_alloy_log);
            assert_eq!(len, alloy_len);
            assert_eq!(compacted_log, compacted_alloy_log);
        }
    }
}
