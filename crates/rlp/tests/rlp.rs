use bytes::{Bytes, BytesMut};
use ethnum::U256;
use hex_literal::hex;
use reth_rlp::{DecodeError, *};

#[derive(Debug, PartialEq, RlpEncodable, RlpDecodable)]
struct Item {
    a: Bytes,
}

#[derive(Debug, PartialEq, RlpEncodable, RlpDecodable, RlpMaxEncodedLen)]
struct Test4Numbers {
    a: u8,
    b: u64,
    c: U256,
    #[rlp(skip)]
    #[rlp(default)]
    s: U256,
    d: U256,
}

#[derive(Debug, PartialEq, RlpEncodableWrapper, RlpDecodableWrapper)]
pub struct W(Test4Numbers);

#[derive(Debug, PartialEq, RlpEncodable)]
struct Test4NumbersGenerics<'a, D: Encodable> {
    a: u8,
    b: u64,
    c: &'a U256,
    d: &'a D,
}

#[derive(Debug, PartialEq, RlpEncodable, RlpDecodable)]
#[rlp(trailing)]
struct TestOpt {
    a: u8,
    b: u64,
    c: Option<u8>,
    d: Option<u64>,
}

fn encoded<T: Encodable>(t: &T) -> BytesMut {
    let mut out = BytesMut::new();
    t.encode(&mut out);
    out
}

#[test]
fn test_encode_item() {
    let item = Item { a: b"dog".to_vec().into() };

    let expected = vec![0xc4, 0x83, b'd', b'o', b'g'];
    let out = encoded(&item);
    assert_eq!(&*out, expected);

    let decoded = Decodable::decode(&mut &*expected).expect("decode failure");
    assert_eq!(item, decoded);

    let item = Test4Numbers {
        a: 0x05,
        b: 0xdeadbeefbaadcafe,
        c: U256::from_be_bytes(hex!(
            "56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421"
        )),
        s: U256::from_be_bytes(hex!(
            "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
        )),
        d: U256::from_be_bytes(hex!(
            "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
        )),
    };

    let expected = hex!("f84c0588deadbeefbaadcafea056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec();
    let out = encoded(&item);
    assert_eq!(&*out, expected);

    let out = reth_rlp::encode_fixed_size(&item);
    assert_eq!(&*out, expected);

    let decoded: Test4Numbers = Decodable::decode(&mut &*expected).unwrap();
    assert_eq!(decoded.a, item.a);
    assert_eq!(decoded.b, item.b);
    assert_eq!(decoded.c, item.c);
    assert_eq!(decoded.d, item.d);
    assert_eq!(decoded.s, U256::ZERO);

    let mut rlp_view = Rlp::new(&expected).unwrap();
    assert_eq!(rlp_view.get_next().unwrap(), Some(item.a));
    assert_eq!(rlp_view.get_next().unwrap(), Some(item.b));
    assert_eq!(rlp_view.get_next().unwrap(), Some(item.c));
    assert_eq!(rlp_view.get_next().unwrap(), Some(item.d));
    assert_eq!(rlp_view.get_next::<Bytes>().unwrap(), None);

    assert_eq!(
        encoded(&Test4NumbersGenerics { a: item.a, b: item.b, c: &item.c, d: &item.d }),
        expected
    );

    assert_eq!(encoded(&W(item)), expected);
    assert_eq!(W::decode(&mut &*expected).unwrap().0, decoded);
    assert_eq!(Test4Numbers::LEN, 79);
}

#[test]
fn invalid_decode_sideeffect() {
    let fixture = hex!("f84d0588deadbeefbaadcafea056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470");
    let mut sl: &[u8] = &fixture;

    assert_eq!(Test4Numbers::decode(&mut sl), Err(DecodeError::InputTooShort));

    assert_eq!(sl.len(), fixture.len());
}

#[test]
fn test_opt_fields_roundtrip() {
    let expected = hex!("c20102");
    let item = TestOpt { a: 1, b: 2, c: None, d: None };
    assert_eq!(&*encoded(&item), expected);
    assert_eq!(TestOpt::decode(&mut &expected[..]).unwrap(), item);

    let expected = hex!("c3010203");
    let item = TestOpt { a: 1, b: 2, c: Some(3), d: None };
    assert_eq!(&*encoded(&item), expected);
    assert_eq!(TestOpt::decode(&mut &expected[..]).unwrap(), item);

    let expected = hex!("c401020304");
    let item = TestOpt { a: 1, b: 2, c: Some(3), d: Some(4) };
    assert_eq!(&*encoded(&item), expected);
    assert_eq!(TestOpt::decode(&mut &expected[..]).unwrap(), item);
}
