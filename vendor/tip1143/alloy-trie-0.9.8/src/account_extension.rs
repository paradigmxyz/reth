//! Shared chain-specific account payloads.

use alloc::vec::Vec;
use alloy_primitives::Bytes;
use core::{cmp::Ordering, ops::Deref};
use triomphe::ThinArc;

/// Raw, unencoded account bytes with a one-pointer inline representation.
///
/// Empty payloads allocate nothing. Nonempty payloads store their length and bytes
/// in one reference-counted allocation; cloning shares that allocation.
///
/// Accounts omit empty extensions in Serde. Their binary representation requires
/// struct boundaries, as in MessagePack; bincode and Postcard are not supported.
#[derive(Clone, Debug, Default)]
pub struct AccountExtension(Option<ThinArc<u8, u8>>);

impl AccountExtension {
    /// Takes ownership of a shared payload without copying its bytes.
    pub fn from_shared(payload: Option<ThinArc<u8, u8>>) -> Self {
        if let Some(arc) = &payload {
            assert!(arc.header.header <= 1, "unknown account extension kind");
            if arc.header.header == 1 {
                assert!(arc.slice.len() >= 4, "invalid typed account extension");
                let len = u32::from_be_bytes(arc.slice[..4].try_into().unwrap()) as usize;
                assert!(len < arc.slice.len() - 4, "missing typed code metadata");
                assert!(
                    arc.slice.len() - 4 - len <= 4096,
                    "typed code metadata exceeds limit"
                );
            }
        }
        Self(payload.filter(|arc| !arc.slice.is_empty()))
    }

    /// Transfers the shared allocation without copying its bytes.
    pub fn into_shared(self) -> Option<ThinArc<u8, u8>> {
        self.0
    }

    /// Creates an empty payload without allocating.
    pub const fn new() -> Self {
        Self(None)
    }

    /// Copies bytes into a single shared allocation.
    pub fn copy_from_slice(bytes: &[u8]) -> Self {
        Self((!bytes.is_empty()).then(|| ThinArc::from_header_and_slice(0, bytes)))
    }

    /// Returns the explicitly typed code metadata, separate from opaque chain fields.
    pub fn code_metadata(&self) -> Option<&[u8]> {
        let arc = self.0.as_ref()?;
        if arc.header.header != 1 {
            return None;
        }
        let len = u32::from_be_bytes(arc.slice[..4].try_into().unwrap()) as usize;
        Some(&arc.slice[4 + len..])
    }

    /// Attaches code metadata without interpreting or replacing existing opaque fields.
    pub fn with_code_metadata(self, metadata: &[u8]) -> Self {
        assert!(!metadata.is_empty(), "typed code metadata cannot be empty");
        assert!(metadata.len() <= 4096, "typed code metadata exceeds limit");
        let opaque = self.as_ref();
        let len = u32::try_from(opaque.len()).expect("opaque account extension exceeds limit");
        let mut bytes = Vec::with_capacity(4 + opaque.len() + metadata.len());
        bytes.extend_from_slice(&len.to_be_bytes());
        bytes.extend_from_slice(opaque);
        bytes.extend_from_slice(metadata);
        Self(Some(ThinArc::from_header_and_slice(1, &bytes)))
    }

    /// Removes code metadata while retaining opaque chain fields.
    pub fn clear_code_metadata(&mut self) {
        if self.code_metadata().is_some() {
            *self = Self::copy_from_slice(self.as_ref());
        }
    }

    /// Returns whether the payload is empty.
    pub const fn is_empty(&self) -> bool {
        self.0.is_none()
    }
}

impl AsRef<[u8]> for AccountExtension {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref().map_or(&[], |arc| {
            if arc.header.header == 1 {
                let len = u32::from_be_bytes(arc.slice[..4].try_into().unwrap()) as usize;
                &arc.slice[4..4 + len]
            } else {
                &arc.slice
            }
        })
    }
}

impl Deref for AccountExtension {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.as_ref()
    }
}

impl From<Bytes> for AccountExtension {
    fn from(bytes: Bytes) -> Self {
        Self::copy_from_slice(&bytes)
    }
}

impl From<Vec<u8>> for AccountExtension {
    fn from(bytes: Vec<u8>) -> Self {
        Self::copy_from_slice(&bytes)
    }
}

impl PartialEq for AccountExtension {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for AccountExtension {}

#[cfg(feature = "borsh")]
impl borsh::BorshSerialize for AccountExtension {
    fn serialize<W: borsh::io::Write>(&self, writer: &mut W) -> borsh::io::Result<()> {
        if self.code_metadata().is_some() {
            return Err(borsh::io::Error::new(
                borsh::io::ErrorKind::InvalidInput,
                "typed account extensions require a self-describing encoding",
            ));
        }
        self.as_ref().serialize(writer)
    }
}

#[cfg(feature = "borsh")]
impl borsh::BorshDeserialize for AccountExtension {
    fn deserialize_reader<R: borsh::io::Read>(reader: &mut R) -> borsh::io::Result<Self> {
        <Vec<u8>>::deserialize_reader(reader).map(Self::from)
    }
}

impl PartialOrd for AccountExtension {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for AccountExtension {
    fn cmp(&self, other: &Self) -> Ordering {
        // Order payload bytes, not ThinArc's length header.
        self.as_ref()
            .cmp(other.as_ref())
            .then_with(|| self.code_metadata().cmp(&other.code_metadata()))
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for AccountExtension {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if let Some(code_metadata) = self.code_metadata() {
            use serde::ser::SerializeMap;
            let mut value = serializer.serialize_map(Some(3))?;
            value.serialize_entry("version", &1u8)?;
            value.serialize_entry("opaque", &Bytes::copy_from_slice(self.as_ref()))?;
            value.serialize_entry("codeMetadata", &Bytes::copy_from_slice(code_metadata))?;
            return value.end();
        }
        if serializer.is_human_readable() {
            alloy_primitives::hex::serialize(self.as_ref(), serializer)
        } else {
            serializer.serialize_bytes(self.as_ref())
        }
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for AccountExtension {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ExtensionVisitor;
        impl<'de> serde::de::Visitor<'de> for ExtensionVisitor {
            type Value = AccountExtension;
            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str("opaque bytes or a versioned account extension")
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                value
                    .parse::<Bytes>()
                    .map(AccountExtension::from)
                    .map_err(E::custom)
            }
            fn visit_bytes<E: serde::de::Error>(self, value: &[u8]) -> Result<Self::Value, E> {
                Ok(AccountExtension::copy_from_slice(value))
            }
            fn visit_byte_buf<E: serde::de::Error>(self, value: Vec<u8>) -> Result<Self::Value, E> {
                Ok(AccountExtension::from(value))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let mut bytes = Vec::new();
                while let Some(byte) = seq.next_element::<u8>()? {
                    bytes.push(byte);
                }
                Ok(AccountExtension::from(bytes))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut version = None;
                let mut opaque = None;
                let mut metadata = None;
                while let Some(key) = map.next_key::<alloc::string::String>()? {
                    match key.as_str() {
                        "version" if version.is_none() => version = Some(map.next_value::<u8>()?),
                        "opaque" if opaque.is_none() => opaque = Some(map.next_value::<Bytes>()?),
                        "codeMetadata" if metadata.is_none() => {
                            metadata = Some(map.next_value::<Bytes>()?)
                        }
                        _ => return Err(serde::de::Error::custom("invalid typed account field")),
                    }
                }
                if version != Some(1) {
                    return Err(serde::de::Error::custom(
                        "unsupported account extension version",
                    ));
                }
                let opaque = opaque.ok_or_else(|| serde::de::Error::missing_field("opaque"))?;
                let metadata =
                    metadata.ok_or_else(|| serde::de::Error::missing_field("codeMetadata"))?;
                if metadata.is_empty() || metadata.len() > 4096 {
                    return Err(serde::de::Error::custom("invalid code metadata length"));
                }
                Ok(AccountExtension::from(opaque).with_code_metadata(&metadata))
            }
        }
        if deserializer.is_human_readable() {
            deserializer.deserialize_any(ExtensionVisitor)
        } else {
            deserializer.deserialize_bytes(ExtensionVisitor)
        }
    }
}

#[cfg(feature = "arbitrary")]
impl<'a> arbitrary::Arbitrary<'a> for AccountExtension {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let bytes = <Vec<u8> as arbitrary::Arbitrary>::arbitrary(u)?;
        Ok(Self::from(bytes))
    }
}

#[cfg(all(test, feature = "serde"))]
mod tests {
    use super::*;

    #[test]
    fn raw_bytes_wire_format() {
        for payload in [&[][..], &[0x82, 0xaa][..], &[42; 256][..]] {
            let extension = AccountExtension::copy_from_slice(payload);
            let expected = postcard::to_allocvec(&Bytes::copy_from_slice(payload)).unwrap();
            let encoded = postcard::to_allocvec(&extension).unwrap();
            assert_eq!(encoded, expected);
            assert_eq!(
                postcard::from_bytes::<AccountExtension>(&encoded).unwrap(),
                extension
            );
            let pair = (extension.clone(), 42u8);
            assert_eq!(
                postcard::from_bytes::<(AccountExtension, u8)>(
                    &postcard::to_allocvec(&pair).unwrap()
                )
                .unwrap(),
                pair
            );
            let json = serde_json::to_string(&extension).unwrap();
            assert_eq!(
                serde_json::from_str::<AccountExtension>(&json).unwrap(),
                extension
            );
            if !payload.is_empty() {
                assert!(
                    postcard::from_bytes::<AccountExtension>(&encoded[..encoded.len() - 1])
                        .is_err()
                );
            }
        }
    }
}

#[cfg(all(test, feature = "borsh"))]
mod borsh_tests {
    use super::*;

    #[test]
    fn uses_standard_byte_vector_encoding() {
        let extension = AccountExtension::copy_from_slice(&[0x82, 0xaa]);
        let encoded = borsh::to_vec(&extension).unwrap();
        assert_eq!(encoded, borsh::to_vec(&vec![0x82u8, 0xaa]).unwrap());
        assert_eq!(
            borsh::from_slice::<AccountExtension>(&encoded).unwrap(),
            extension
        );
    }
}

impl AccountExtension {
    /// Length of the explicitly selected extension wire representation.
    pub(crate) fn rlp_length(&self) -> usize {
        use alloy_rlp::Encodable;
        if let Some(code) = self.code_metadata() {
            let payload_length = 1u8.length() + self.as_ref().length() + code.length();
            alloy_rlp::Header {
                list: true,
                payload_length,
            }
            .length()
                + payload_length
        } else {
            self.as_ref().length()
        }
    }

    /// Encodes typed fields as a list, disjoint from every legacy opaque string.
    pub(crate) fn rlp_encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        use alloy_rlp::Encodable;
        if let Some(code) = self.code_metadata() {
            let payload_length = 1u8.length() + self.as_ref().length() + code.length();
            alloy_rlp::Header {
                list: true,
                payload_length,
            }
            .encode(out);
            1u8.encode(out);
            self.as_ref().encode(out);
            code.encode(out);
        } else {
            self.as_ref().encode(out);
        }
    }

    /// Decodes the list/string discriminator before examining any payload bytes.
    pub(crate) fn rlp_decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        use alloy_rlp::{Decodable, Error, Header};
        if buf.first().is_none_or(|value| *value < 0xc0) {
            let bytes = Header::decode_bytes(buf, false)?;
            if bytes.is_empty() {
                return Err(Error::Custom("empty account extension must be omitted"));
            }
            return Ok(Self::copy_from_slice(bytes));
        }
        let mut fields = Header::decode_bytes(buf, true)?;
        if u8::decode(&mut fields)? != 1 {
            return Err(Error::Custom("unsupported account extension version"));
        }
        let opaque = Header::decode_bytes(&mut fields, false)?;
        let code = Header::decode_bytes(&mut fields, false)?;
        if !fields.is_empty() || code.is_empty() || code.len() > 4096 {
            return Err(Error::Custom("invalid typed account extension"));
        }
        Ok(Self::copy_from_slice(opaque).with_code_metadata(code))
    }
}
