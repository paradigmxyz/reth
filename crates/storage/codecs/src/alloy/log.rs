use crate::Compact;
use alloy_primitives::{Address, Bytes, Log, LogData};
use bytes::BufMut;

/// Implement `Compact` for `LogData` and `Log`.
impl Compact for LogData {
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: BufMut + AsMut<[u8]>,
    {
        let mut buffer = bytes::BytesMut::new();
        self.topics().to_vec().specialized_to_compact(&mut buffer);
        self.data.to_compact(&mut buffer);
        let total_length = buffer.len();
        buf.put(buffer);
        total_length
    }

    fn from_compact(mut buf: &[u8], _: usize) -> (Self, &[u8]) {
        let (topics, new_buf) = Vec::specialized_from_compact(buf, buf.len());
        buf = new_buf;
        let (data, buf) = Bytes::from_compact(buf, buf.len());
        let log_data = LogData::new_unchecked(topics, data);
        (log_data, buf)
    }
}

impl Compact for Log {
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: BufMut + AsMut<[u8]>,
    {
        let mut buffer = bytes::BytesMut::new();
        self.address.to_compact(&mut buffer);
        self.data.to_compact(&mut buffer);
        let total_length = buffer.len();
        buf.put(buffer);
        total_length
    }

    fn from_compact(mut buf: &[u8], _: usize) -> (Self, &[u8]) {
        let (address, new_buf) = Address::from_compact(buf, buf.len());
        buf = new_buf;
        let (log_data, new_buf) = LogData::from_compact(buf, buf.len());
        buf = new_buf;
        let log = Log { address, data: log_data };
        (log, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::{Compact, Log};
    use alloy_primitives::LogData;
    use proptest::proptest;

    proptest! {
        #[test]
        fn roundtrip(log: Log) {
            let mut buf = Vec::<u8>::new();
            let len = log.clone().to_compact(&mut buf);
            let (decoded, _) = Log::from_compact(&buf, len);
            assert_eq!(log, decoded);
        }
    }

    #[test]
    fn test_against_reth_log() {
        // log: Log { address: 0x77aa337cd3426acadddacc4fa0d31ec4a619fd37, topics:
        // [0x86c887f5d68d1213db3973e26710f0e0af1551b44d3509c8827f78bc50523018,
        // 0x4aad6d3d3a104abe5d1f082f811d4ea799b76294f36bec1634153cbea437315e,
        // 0xe7b75d801564118894c62675cfb8d67f268529f96b9ca2491ecb6113488e3286,
        // 0x2a07ac81989c47dee275e034c91272c493e8df0330fb696f9b9472aa936facad,
        // 0xeca38d64f2f590dd312906fdf6a3c92cd4e2bac0aed7bed0069ea97c755f3ad0], data:
        // 0xd83df033231aac75dbe158aa34a8417da85b897ee3c3fe5a6abe6a6f73bbfc9ad2281c154c1063bab6086e53de6fa10cdb965e08934614d7c3e0d960a2441760047b1ce8fe1a19c75dcc4f9da3b67740fc016f4a2c1926ffb43a642623
        // }, encoded:[119, 170, 51, 124, 211, 66, 106, 202, 221, 218, 204, 79, 160, 211, 30, 196,
        // 166, 25, 253, 55, 5, 134, 200, 135, 245, 214, 141, 18, 19, 219, 57, 115, 226, 103, 16,
        // 240, 224, 175, 21, 81, 180, 77, 53, 9, 200, 130, 127, 120, 188, 80, 82, 48, 24, 74, 173,
        // 109, 61, 58, 16, 74, 190, 93, 31, 8, 47, 129, 29, 78, 167, 153, 183, 98, 148, 243, 107,
        // 236, 22, 52, 21, 60, 190, 164, 55, 49, 94, 231, 183, 93, 128, 21, 100, 17, 136, 148, 198,
        // 38, 117, 207, 184, 214, 127, 38, 133, 41, 249, 107, 156, 162, 73, 30, 203, 97, 19, 72,
        // 142, 50, 134, 42, 7, 172, 129, 152, 156, 71, 222, 226, 117, 224, 52, 201, 18, 114, 196,
        // 147, 232, 223, 3, 48, 251, 105, 111, 155, 148, 114, 170, 147, 111, 172, 173, 236, 163,
        // 141, 100, 242, 245, 144, 221, 49, 41, 6, 253, 246, 163, 201, 44, 212, 226, 186, 192, 174,
        // 215, 190, 208, 6, 158, 169, 124, 117, 95, 58, 208, 216, 61, 240, 51, 35, 26, 172, 117,
        // 219, 225, 88, 170, 52, 168, 65, 125, 168, 91, 137, 126, 227, 195, 254, 90, 106, 190, 106,
        // 111, 115, 187, 252, 154, 210, 40, 28, 21, 76, 16, 99, 186, 182, 8, 110, 83, 222, 111,
        // 161, 12, 219, 150, 94, 8, 147, 70, 20, 215, 195, 224, 217, 96, 162, 68, 23, 96, 4, 123,
        // 28, 232, 254, 26, 25, 199, 93, 204, 79, 157, 163, 182, 119, 64, 252, 1, 111, 74, 44, 25,
        // 38, 255, 180, 58, 100, 38, 35]
        let topics = vec![
            "0x86c887f5d68d1213db3973e26710f0e0af1551b44d3509c8827f78bc50523018".parse().unwrap(),
            "0x4aad6d3d3a104abe5d1f082f811d4ea799b76294f36bec1634153cbea437315e".parse().unwrap(),
            "0xe7b75d801564118894c62675cfb8d67f268529f96b9ca2491ecb6113488e3286".parse().unwrap(),
            "0x2a07ac81989c47dee275e034c91272c493e8df0330fb696f9b9472aa936facad".parse().unwrap(),
            "0xeca38d64f2f590dd312906fdf6a3c92cd4e2bac0aed7bed0069ea97c755f3ad0".parse().unwrap(),
        ];
        let data = "0xd83df033231aac75dbe158aa34a8417da85b897ee3c3fe5a6abe6a6f73bbfc9ad2281c154c1063bab6086e53de6fa10cdb965e08934614d7c3e0d960a2441760047b1ce8fe1a19c75dcc4f9da3b67740fc016f4a2c1926ffb43a642623".parse().unwrap();
        let log_data = LogData::new_unchecked(topics, data);
        let log = Log {
            address: "0x77aa337cd3426acadddacc4fa0d31ec4a619fd37".parse().unwrap(),
            data: log_data,
        };

        let mut buf = Vec::<u8>::new();
        let len = log.clone().to_compact(&mut buf);
        let (decoded, _) = Log::from_compact(&buf, len);
        let output_value = vec![
            119, 170, 51, 124, 211, 66, 106, 202, 221, 218, 204, 79, 160, 211, 30, 196, 166, 25,
            253, 55, 5, 134, 200, 135, 245, 214, 141, 18, 19, 219, 57, 115, 226, 103, 16, 240, 224,
            175, 21, 81, 180, 77, 53, 9, 200, 130, 127, 120, 188, 80, 82, 48, 24, 74, 173, 109, 61,
            58, 16, 74, 190, 93, 31, 8, 47, 129, 29, 78, 167, 153, 183, 98, 148, 243, 107, 236, 22,
            52, 21, 60, 190, 164, 55, 49, 94, 231, 183, 93, 128, 21, 100, 17, 136, 148, 198, 38,
            117, 207, 184, 214, 127, 38, 133, 41, 249, 107, 156, 162, 73, 30, 203, 97, 19, 72, 142,
            50, 134, 42, 7, 172, 129, 152, 156, 71, 222, 226, 117, 224, 52, 201, 18, 114, 196, 147,
            232, 223, 3, 48, 251, 105, 111, 155, 148, 114, 170, 147, 111, 172, 173, 236, 163, 141,
            100, 242, 245, 144, 221, 49, 41, 6, 253, 246, 163, 201, 44, 212, 226, 186, 192, 174,
            215, 190, 208, 6, 158, 169, 124, 117, 95, 58, 208, 216, 61, 240, 51, 35, 26, 172, 117,
            219, 225, 88, 170, 52, 168, 65, 125, 168, 91, 137, 126, 227, 195, 254, 90, 106, 190,
            106, 111, 115, 187, 252, 154, 210, 40, 28, 21, 76, 16, 99, 186, 182, 8, 110, 83, 222,
            111, 161, 12, 219, 150, 94, 8, 147, 70, 20, 215, 195, 224, 217, 96, 162, 68, 23, 96, 4,
            123, 28, 232, 254, 26, 25, 199, 93, 204, 79, 157, 163, 182, 119, 64, 252, 1, 111, 74,
            44, 25, 38, 255, 180, 58, 100, 38, 35,
        ];

        let mut asserted_buf = vec![];
        for byte in output_value {
            asserted_buf.push(byte as u8);
        }

        assert_eq!(buf, asserted_buf);
    }
}
