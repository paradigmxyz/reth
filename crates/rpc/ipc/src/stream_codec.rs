// Copyright (c) 2015-2017 Parity Technologies Limited
//
// Permission is hereby granted, free of charge, to any
// person obtaining a copy of this software and associated
// documentation files (the "Software"), to deal in the
// Software without restriction, including without
// limitation the rights to use, copy, modify, merge,
// publish, distribute, sublicense, and/or sell copies of
// the Software, and to permit persons to whom the Software
// is furnished to do so, subject to the following
// conditions:
//
// The above copyright notice and this permission notice
// shall be included in all copies or substantial portions
// of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
// ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
// TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
// PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
// SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
// CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
// OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
// IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

// This basis of this file has been taken from the deprecated jsonrpc codebase:
// https://github.com/paritytech/jsonrpc

use bytes::BytesMut;
use std::{io, str};

/// Separator for enveloping messages in streaming codecs
#[derive(Debug, Clone)]
pub enum Separator {
    /// No envelope is expected between messages. Decoder will try to figure out
    /// message boundaries by accumulating incoming bytes until valid JSON is formed.
    /// Encoder will send messages without any boundaries between requests.
    Empty,
    /// Byte is used as a sentinel between messages
    Byte(u8),
}

impl Default for Separator {
    fn default() -> Self {
        Separator::Byte(b'\n')
    }
}

/// Stream codec for streaming protocols (ipc, tcp)
#[derive(Debug, Default)]
pub struct StreamCodec {
    incoming_separator: Separator,
    outgoing_separator: Separator,
}

impl StreamCodec {
    /// Default codec with streaming input data. Input can be both enveloped and not.
    pub fn stream_incoming() -> Self {
        StreamCodec::new(Separator::Empty, Default::default())
    }

    /// New custom stream codec
    pub fn new(incoming_separator: Separator, outgoing_separator: Separator) -> Self {
        StreamCodec { incoming_separator, outgoing_separator }
    }
}

#[inline]
fn is_whitespace(byte: u8) -> bool {
    matches!(byte, 0x0D | 0x0A | 0x20 | 0x09)
}

impl tokio_util::codec::Decoder for StreamCodec {
    type Item = String;
    type Error = io::Error;

    fn decode(&mut self, buf: &mut BytesMut) -> io::Result<Option<Self::Item>> {
        if let Separator::Byte(separator) = self.incoming_separator {
            if let Some(i) = buf.as_ref().iter().position(|&b| b == separator) {
                let line = buf.split_to(i);
                let _ = buf.split_to(1);

                match str::from_utf8(line.as_ref()) {
                    Ok(s) => Ok(Some(s.to_string())),
                    Err(_) => Err(io::Error::new(io::ErrorKind::Other, "invalid UTF-8")),
                }
            } else {
                Ok(None)
            }
        } else {
            let mut depth = 0;
            let mut in_str = false;
            let mut is_escaped = false;
            let mut start_idx = 0;
            let mut whitespaces = 0;

            for idx in 0..buf.as_ref().len() {
                let byte = buf.as_ref()[idx];

                if (byte == b'{' || byte == b'[') && !in_str {
                    if depth == 0 {
                        start_idx = idx;
                    }
                    depth += 1;
                } else if (byte == b'}' || byte == b']') && !in_str {
                    depth -= 1;
                } else if byte == b'"' && !is_escaped {
                    in_str = !in_str;
                } else if is_whitespace(byte) {
                    whitespaces += 1;
                }
                is_escaped = byte == b'\\' && !is_escaped && in_str;

                if depth == 0 && idx != start_idx && idx - start_idx + 1 > whitespaces {
                    let bts = buf.split_to(idx + 1);
                    return match String::from_utf8(bts.as_ref().to_vec()) {
                        Ok(val) => Ok(Some(val)),
                        Err(_) => Ok(None),
                    }
                }
            }
            Ok(None)
        }
    }
}

impl tokio_util::codec::Encoder<String> for StreamCodec {
    type Error = io::Error;

    fn encode(&mut self, msg: String, buf: &mut BytesMut) -> io::Result<()> {
        let mut payload = msg.into_bytes();
        if let Separator::Byte(separator) = self.outgoing_separator {
            payload.push(separator);
        }
        buf.extend_from_slice(&payload);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::{BufMut, BytesMut};
    use tokio_util::codec::Decoder;

    #[test]
    fn simple_encode() {
        let mut buf = BytesMut::with_capacity(2048);
        buf.put_slice(b"{ test: 1 }{ test: 2 }{ test: 3 }");

        let mut codec = StreamCodec::stream_incoming();

        let request = codec
            .decode(&mut buf)
            .expect("There should be no error in simple test")
            .expect("There should be at least one request in simple test");

        assert_eq!(request, "{ test: 1 }");
    }

    #[test]
    fn escape() {
        let mut buf = BytesMut::with_capacity(2048);
        buf.put_slice(br#"{ test: "\"\\" }{ test: "\ " }{ test: "\}" }[ test: "\]" ]"#);

        let mut codec = StreamCodec::stream_incoming();

        let request = codec
            .decode(&mut buf)
            .expect("There should be no error in first escape test")
            .expect("There should be a request in first escape test");

        assert_eq!(request, r#"{ test: "\"\\" }"#);

        let request2 = codec
            .decode(&mut buf)
            .expect("There should be no error in 2nd escape test")
            .expect("There should be a request in 2nd escape test");
        assert_eq!(request2, r#"{ test: "\ " }"#);

        let request3 = codec
            .decode(&mut buf)
            .expect("There should be no error in 3rd escape test")
            .expect("There should be a request in 3rd escape test");
        assert_eq!(request3, r#"{ test: "\}" }"#);

        let request4 = codec
            .decode(&mut buf)
            .expect("There should be no error in 4th escape test")
            .expect("There should be a request in 4th escape test");
        assert_eq!(request4, r#"[ test: "\]" ]"#);
    }

    #[test]
    fn whitespace() {
        let mut buf = BytesMut::with_capacity(2048);
        buf.put_slice(b"{ test: 1 }\n\n\n\n{ test: 2 }\n\r{\n test: 3 }  ");

        let mut codec = StreamCodec::stream_incoming();

        let request = codec
            .decode(&mut buf)
            .expect("There should be no error in first whitespace test")
            .expect("There should be a request in first whitespace test");

        assert_eq!(request, "{ test: 1 }");

        let request2 = codec
            .decode(&mut buf)
            .expect("There should be no error in first 2nd test")
            .expect("There should be aa request in 2nd whitespace test");
        // TODO: maybe actually trim it out
        assert_eq!(request2, "\n\n\n\n{ test: 2 }");

        let request3 = codec
            .decode(&mut buf)
            .expect("There should be no error in first 3rd test")
            .expect("There should be a request in 3rd whitespace test");
        assert_eq!(request3, "\n\r{\n test: 3 }");

        let request4 = codec.decode(&mut buf).expect("There should be no error in first 4th test");
        assert!(
            request4.is_none(),
            "There should be no 4th request because it contains only whitespaces"
        );
    }

    #[test]
    fn fragmented_encode() {
        let mut buf = BytesMut::with_capacity(2048);
        buf.put_slice(b"{ test: 1 }{ test: 2 }{ tes");

        let mut codec = StreamCodec::stream_incoming();

        let request = codec
            .decode(&mut buf)
            .expect("There should be no error in first fragmented test")
            .expect("There should be at least one request in first fragmented test");
        assert_eq!(request, "{ test: 1 }");
        codec
            .decode(&mut buf)
            .expect("There should be no error in second fragmented test")
            .expect("There should be at least one request in second fragmented test");
        assert_eq!(String::from_utf8(buf.as_ref().to_vec()).unwrap(), "{ tes");

        buf.put_slice(b"t: 3 }");
        let request = codec
            .decode(&mut buf)
            .expect("There should be no error in third fragmented test")
            .expect("There should be at least one request in third fragmented test");
        assert_eq!(request, "{ test: 3 }");
    }

    #[test]
    fn huge() {
        let request = r#"
		{
			"jsonrpc":"2.0",
			"method":"say_hello",
			"params": [
				42,
				0,
				{
					"from":"0xb60e8dd61c5d32be8058bb8eb970870f07233155",
					"gas":"0x2dc6c0",
					"data":"0x606060405260003411156010576002565b6001805433600160a060020a0319918216811790925560028054909116909117905561291f806100406000396000f3606060405236156100e55760e060020a600035046304029f2381146100ed5780630a1273621461015f57806317c1dd87146102335780631f9ea25d14610271578063266fa0e91461029357806349593f5314610429578063569aa0d8146104fc57806359a4669f14610673578063647a4d5f14610759578063656104f5146108095780636e9febfe1461082b57806370de8c6e1461090d57806371bde852146109ed5780638f30435d14610ab4578063916dbc1714610da35780639f5a7cd414610eef578063c91540f614610fe6578063eae99e1c146110b5578063fedc2a281461115a575b61122d610002565b61122d6004808035906020019082018035906020019191908080601f01602080910402602001604051908101604052809392919081815260200183838082843750949650509335935050604435915050606435600154600090600160a060020a03908116339091161461233357610002565b61122f6004808035906020019082018035906020019191908080601f016020809104026020016040519081016040528093929190818152602001838380828437509496505093359350506044359150506064355b60006000600060005086604051808280519060200190808383829060006004602084601f0104600f02600301f1509050019150509081526020016040518091039020600050905042816005016000508560ff1660028110156100025760040201835060010154604060020a90046001604060020a0316116115df576115d6565b6112416004355b604080516001604060020a038316408152606060020a33600160a060020a031602602082015290519081900360340190205b919050565b61122d600435600254600160a060020a0390811633909116146128e357610002565b61125e6004808035906020019082018035906020019191908080601f01602080910402602001604051908101604052809392919081815260200183838082843750949650509335935050505060006000600060006000600060005087604051808280519060200190808383829060006004602084601f0104600f02600301f1509050019150509081526020016040518091039020600050905080600001600050600087600160a060020a0316815260200190815260200160002060005060000160059054906101000a90046001604060020a03169450845080600001600050600087600160a060020a03168152602001908152602001600020600050600001600d9054906101000a90046001604060020a03169350835080600001600050600087600160a060020a0316815260200190815260200160002060005060000160009054906101000a900460ff169250825080600001600050600087600160a060020a0316815260200190815260200160002060005060000160019054906101000a900463ffffffff16915081505092959194509250565b61122d6004808035906020019082018035906020019191908080601f01602080910402602001604051908101604052809392919081815260200183838082843750949650509335935050604435915050606435608435600060006000600060005088604051808280519060200190808383829060006004602084601f0104600f02600301f15090500191505090815260200160405180910390206000509250346000141515611c0e5760405133600160a060020a0316908290349082818181858883f193505050501515611c1a57610002565b6112996004808035906020019082018035906020019191908080601f01602080910402602001604051908101604052809392919081815260200183838082843750949650509335935050604435915050600060006000600060006000600060006000508a604051808280519060200190808383829060006004602084601f0104600f02600301f15090500191505090815260200160405180910390206000509050806001016000508960ff16600281101561000257600160a060020a038a168452828101600101602052604084205463ffffffff1698506002811015610002576040842054606060020a90046001604060020a031697506002811015610002576040842054640100000000900463ffffffff169650600281101561000257604084206001015495506002811015610002576040842054604060020a900463ffffffff169450600281101561000257505060409091205495999498509296509094509260a060020a90046001604060020a0316919050565b61122d6004808035906020019082018035906020019191908080601f016020809104026020016040519081016040528093929190818152602001838380828437509496505050505050506000600060005082604051808280519060200190808383829060006004602084601f0104600f02600301f15090500191505090815260200160405180910390206000509050348160050160005082600d0160009054906101000a900460ff1660ff16600281101561000257600402830160070180546001608060020a0381169093016001608060020a03199390931692909217909155505b5050565b6112e26004808035906020019082018035906020019191908080601f01602080910003423423094734987103498712093847102938740192387401349857109487501938475"
				}
			]
		}"#;

        let mut buf = BytesMut::with_capacity(65536);
        buf.put_slice(request.as_bytes());

        let mut codec = StreamCodec::stream_incoming();

        let parsed_request = codec
            .decode(&mut buf)
            .expect("There should be no error in huge test")
            .expect("There should be at least one request huge test");
        assert_eq!(request, parsed_request);
    }

    #[test]
    fn simple_line_codec() {
        let mut buf = BytesMut::with_capacity(2048);
        buf.put_slice(b"{ test: 1 }\n{ test: 2 }\n{ test: 3 }");

        let mut codec = StreamCodec::default();

        let request = codec
            .decode(&mut buf)
            .expect("There should be no error in simple test")
            .expect("There should be at least one request in simple test");
        let request2 = codec
            .decode(&mut buf)
            .expect("There should be no error in simple test")
            .expect("There should be at least one request in simple test");

        assert_eq!(request, "{ test: 1 }");
        assert_eq!(request2, "{ test: 2 }");
    }
}
