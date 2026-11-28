//! Shared (EIP-8070) and indexed (EIP-8408) cell retrieval messages.
//!
//! Payload codecs are flattened: `RequestPair` supplies the outer RLP list and request id.

use alloc::vec::Vec;
use alloy_eips::eip7594::{BlobCellMask, Cell};
use alloy_primitives::{map::B256Map, Bytes, B128, B256};
use alloy_rlp::{BufMut, Decodable, Encodable, Error, Header};
use reth_primitives_traits::InMemorySize;

#[cfg(feature = "serde")]
use core::fmt;
#[cfg(feature = "serde")]
use serde::{
    de::{
        value::{SeqAccessDeserializer, StrDeserializer},
        SeqAccess, Visitor,
    },
    Deserialize, Deserializer, Serialize, Serializer,
};

/// Mandatory transaction count bound for indexed cell messages (EIP-8408).
pub const MAX_INDEXED_CELL_HASHES: usize = 64;

/// Mandatory mask table bound for indexed cell messages (EIP-8408).
pub const MAX_INDEXED_CELL_MASKS: usize = 64;

#[cfg(feature = "serde")]
const MAX_SERIALIZED_CELL_MASK_BYTES: usize =
    Header { list: true, payload_length: MAX_INDEXED_CELL_MASKS * (B128::len_bytes() + 1) }
        .length_with_payload() +
        Header { list: false, payload_length: MAX_INDEXED_CELL_HASHES }.length_with_payload();

/// Cell indices shared by all transactions or selected through a canonical mask table.
/// Serde preserves the `B128` representation for shared masks. Indexed masks serialize as
/// their RLP field sequence in a hex string (or bytes for binary formats).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CellMasks {
    /// The eth/72 form, also accepted on eth/73 without indexed-form limits.
    Shared(B128),
    /// The eth/73 form. Masks must be unique, referenced, and ordered by first use.
    Indexed {
        /// At most 64 masks. Requests need at least two; empty responses use none.
        masks: Vec<B128>,
        /// One byte per transaction hash, indexing `masks`.
        mask_ids: Bytes,
    },
}

impl CellMasks {
    /// Returns whether this uses the eth/73 indexed form.
    pub const fn is_indexed(&self) -> bool {
        matches!(self, Self::Indexed { .. })
    }

    /// Returns the shared mask or the mask at an indexed transaction position.
    /// Returns `None` if an indexed identifier or table entry is missing.
    pub fn mask_at(&self, index: usize) -> Option<B128> {
        match self {
            Self::Shared(mask) => Some(*mask),
            Self::Indexed { masks, mask_ids } => masks.get(*mask_ids.get(index)? as usize).copied(),
        }
    }

    /// Validates the indexed bounds, uniqueness, and first-reference ordering.
    /// Shared forms retain eth/72 semantics. Only responses may use an empty indexed table
    /// or an indexed table with a single mask.
    pub fn validate(&self, hashes: &[B256], response: bool) -> alloy_rlp::Result<()> {
        let Self::Indexed { masks, mask_ids } = self else { return Ok(()) };
        if hashes.len() > MAX_INDEXED_CELL_HASHES ||
            masks.len() > MAX_INDEXED_CELL_MASKS ||
            mask_ids.len() != hashes.len() ||
            (!response && hashes.is_empty()) ||
            (hashes.is_empty() != masks.is_empty())
        {
            return Err(Error::Custom("invalid indexed cell counts"))
        }
        let mut next = 0;
        for &id in mask_ids {
            let id = usize::from(id);
            if id >= masks.len() || id > next {
                return Err(Error::Custom("invalid cell mask identifier order"))
            }
            if id == next {
                next += 1;
            }
        }
        if next != masks.len() {
            return Err(Error::Custom("unreferenced cell mask"))
        }
        // Both collections are bounded by 64; no hash table allocation is needed.
        for (index, mask) in masks.iter().enumerate() {
            if masks[..index].contains(mask) {
                return Err(Error::Custom("duplicate cell mask"))
            }
        }
        for (index, hash) in hashes.iter().enumerate() {
            if hashes[..index].contains(hash) {
                return Err(Error::Custom("duplicate cell transaction hash"))
            }
        }
        if !response && masks.len() == 1 {
            return Err(Error::Custom("uniform cell requests must use the shared form"))
        }
        Ok(())
    }
}

impl Default for CellMasks {
    fn default() -> Self {
        Self::Shared(B128::ZERO)
    }
}

impl From<B128> for CellMasks {
    fn from(mask: B128) -> Self {
        Self::Shared(mask)
    }
}

impl Encodable for CellMasks {
    fn encode(&self, out: &mut dyn BufMut) {
        match self {
            Self::Shared(mask) => mask.encode(out),
            Self::Indexed { masks, mask_ids } => {
                masks.encode(out);
                mask_ids.encode(out);
            }
        }
    }

    fn length(&self) -> usize {
        match self {
            Self::Shared(mask) => mask.length(),
            Self::Indexed { masks, mask_ids } => masks.length() + mask_ids.length(),
        }
    }
}

impl Decodable for CellMasks {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let mut header_buf = *buf;
        if !Header::decode(&mut header_buf)?.list {
            return B128::decode(buf).map(Self::Shared)
        }
        let masks = decode_bounded_list(buf, MAX_INDEXED_CELL_MASKS)?;
        let ids = Header::decode_bytes(buf, false)?;
        if ids.len() > MAX_INDEXED_CELL_HASHES {
            return Err(Error::Custom("invalid indexed cell counts"))
        }
        Ok(Self::Indexed { masks, mask_ids: Bytes::copy_from_slice(ids) })
    }
}

#[cfg(feature = "serde")]
impl Serialize for CellMasks {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Shared(mask) => mask.serialize(serializer),
            Self::Indexed { masks, mask_ids } => {
                if masks.len() > MAX_INDEXED_CELL_MASKS || mask_ids.len() > MAX_INDEXED_CELL_HASHES
                {
                    return Err(serde::ser::Error::custom("invalid indexed cell counts"))
                }
                let encoded = alloy_rlp::encode(self);
                // Valid indexed forms are either two bytes (empty) or at least 19 bytes.
                if encoded.len() == B128::len_bytes() {
                    return Err(serde::ser::Error::custom("invalid indexed cell mask encoding"))
                }
                Bytes::from(encoded).serialize(serializer)
            }
        }
    }
}

#[cfg(feature = "serde")]
impl<'de> Deserialize<'de> for CellMasks {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct CellMasksVisitor;

        impl<'de> Visitor<'de> for CellMasksVisitor {
            type Value = CellMasks;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a 16-byte shared mask or bounded indexed cell mask bytes")
            }

            fn visit_bytes<E: serde::de::Error>(self, encoded: &[u8]) -> Result<Self::Value, E> {
                if encoded.len() == B128::len_bytes() {
                    return Ok(CellMasks::Shared(B128::from_slice(encoded)))
                }
                if encoded.len() > MAX_SERIALIZED_CELL_MASK_BYTES {
                    return Err(E::custom("serialized cell masks exceed indexed bounds"))
                }
                let mut buf = encoded;
                let masks = CellMasks::decode(&mut buf).map_err(E::custom)?;
                if !masks.is_indexed() || !buf.is_empty() {
                    return Err(E::custom("invalid indexed cell mask encoding"))
                }
                Ok(masks)
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                let shared_hex_len = B128::len_bytes() * 2;
                if value.len() == shared_hex_len || value.len() == shared_hex_len + 2 {
                    return B128::deserialize(StrDeserializer::<E>::new(value))
                        .map(CellMasks::Shared)
                }
                // Bound the hex input before Bytes allocates its decoding buffer.
                if value.len() > MAX_SERIALIZED_CELL_MASK_BYTES * 2 + 2 {
                    return Err(E::custom("serialized cell masks exceed indexed bounds"))
                }
                let encoded = Bytes::deserialize(StrDeserializer::<E>::new(value))?;
                self.visit_bytes(&encoded)
            }

            fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
                B128::deserialize(SeqAccessDeserializer::new(seq)).map(CellMasks::Shared)
            }
        }

        if deserializer.is_human_readable() {
            deserializer.deserialize_any(CellMasksVisitor)
        } else {
            deserializer.deserialize_bytes(CellMasksVisitor)
        }
    }
}

/// Transaction hashes and the cell indices requested from every blob in each transaction.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GetCells {
    /// Transaction hashes, unique and bounded to 64 in indexed requests.
    pub hashes: Vec<B256>,
    /// Shared or indexed little-endian cell bitvectors.
    pub cell_mask: CellMasks,
}

impl GetCells {
    /// Returns the requested indices at a transaction position, interpreting the wire bitvector.
    pub fn cell_mask(&self, index: usize) -> Option<BlobCellMask> {
        self.cell_mask.mask_at(index).map(BlobCellMask::new)
    }

    /// Builds an adaptive request from one mask per hash, choosing shared for uniform masks.
    /// Rejects mismatched lengths, empty batches, or invalid indexed requests.
    pub fn from_masks(hashes: Vec<B256>, masks: Vec<B128>) -> alloy_rlp::Result<Self> {
        if hashes.is_empty() || hashes.len() != masks.len() {
            return Err(Error::Custom("invalid indexed cell counts"))
        }
        let first = masks[0];
        if masks.iter().all(|mask| *mask == first) {
            return Ok(Self { hashes, cell_mask: first.into() })
        }
        if hashes.len() > MAX_INDEXED_CELL_HASHES {
            return Err(Error::Custom("invalid indexed cell counts"))
        }
        let mut unique = Vec::new();
        let mut ids = Vec::with_capacity(masks.len());
        for mask in masks {
            let id = unique.iter().position(|entry| *entry == mask).unwrap_or_else(|| {
                unique.push(mask);
                unique.len() - 1
            });
            // The hash count bounds the number of distinct masks to 64.
            ids.push(id as u8);
        }
        let cell_mask = CellMasks::Indexed { masks: unique, mask_ids: ids.into() };
        cell_mask.validate(&hashes, false)?;
        Ok(Self { hashes, cell_mask })
    }
}

impl InMemorySize for GetCells {
    fn size(&self) -> usize {
        self.hashes.len() * core::mem::size_of::<B256>() +
            match &self.cell_mask {
                CellMasks::Shared(_) => core::mem::size_of::<B128>(),
                CellMasks::Indexed { masks, mask_ids } => {
                    masks.len() * core::mem::size_of::<B128>() + mask_ids.len()
                }
            }
    }
}

impl Encodable for GetCells {
    fn encode(&self, out: &mut dyn BufMut) {
        self.hashes.encode(out);
        self.cell_mask.encode(out);
    }

    fn length(&self) -> usize {
        self.hashes.length() + self.cell_mask.length()
    }
}

impl Decodable for GetCells {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        // Inspect masks before allocating hashes so indexed counts are bounded during decoding.
        let mut tail = *buf;
        Header::decode_bytes(&mut tail, true)?;
        let cell_mask = CellMasks::decode(&mut tail)?;
        let hashes = if cell_mask.is_indexed() {
            decode_bounded_list(buf, MAX_INDEXED_CELL_HASHES)?
        } else {
            Vec::<B256>::decode(buf)?
        };
        cell_mask.validate(&hashes, false)?;
        *buf = tail;
        Ok(Self { hashes, cell_mask })
    }
}

/// Returned cells, grouped by transaction, then blob order, then increasing set bit index.
/// Cells remain untrusted until checked against the transaction commitments and cell proofs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Cells {
    /// Hashes identifying the returned transactions.
    pub hashes: Vec<B256>,
    /// Cells in blob order and increasing cell-index order within each blob.
    pub cells: Vec<Vec<Cell>>,
    /// Returned cell indices for each transaction.
    pub cell_mask: CellMasks,
}

impl Cells {
    /// Checks form, requested hash multiplicities, mask subsets, and cell-list dimensions.
    /// Cryptographic verification requires the transaction's commitments and proofs and is
    /// performed by the caller before importing any data into the blobpool.
    pub fn validate_response(&self, request: &GetCells) -> alloy_rlp::Result<()> {
        self.cell_mask.validate(&self.hashes, true)?;
        if self.cell_mask.is_indexed() != request.cell_mask.is_indexed() ||
            self.hashes.len() != self.cells.len()
        {
            return Err(Error::Custom("invalid cell response form"))
        }
        // Indexed requests are bounded to 64 hashes. Large shared responses need a lookup
        // table to prevent peer-controlled repeated hashes from causing quadratic work.
        let mut shared_hashes = (matches!(request.cell_mask, CellMasks::Shared(_)) &&
            request.hashes.len() > MAX_INDEXED_CELL_HASHES &&
            self.hashes.len() > MAX_INDEXED_CELL_HASHES)
            .then(|| {
                let mut remaining = B256Map::<usize>::with_capacity_and_hasher(
                    request.hashes.len(),
                    Default::default(),
                );
                for hash in &request.hashes {
                    *remaining.entry(*hash).or_default() += 1;
                }
                remaining
            });
        for (index, hash) in self.hashes.iter().enumerate() {
            let request_index = if let Some(hashes) = &mut shared_hashes {
                let remaining = hashes
                    .get_mut(hash)
                    .ok_or(Error::Custom("unrequested cell transaction hash"))?;
                if *remaining == 0 {
                    return Err(Error::Custom("duplicate cell transaction hash"))
                }
                *remaining -= 1;
                // Shared masks do not depend on the transaction position.
                0
            } else {
                let request_index = request
                    .hashes
                    .iter()
                    .position(|requested| requested == hash)
                    .ok_or(Error::Custom("unrequested cell transaction hash"))?;
                if !self.cell_mask.is_indexed() && self.hashes[..index].contains(hash) {
                    let returned =
                        self.hashes[..index].iter().filter(|entry| *entry == hash).count();
                    let requested = request.hashes.iter().filter(|entry| *entry == hash).count();
                    if returned >= requested {
                        return Err(Error::Custom("duplicate cell transaction hash"))
                    }
                }
                request_index
            };
            let mask = self
                .cell_mask
                .mask_at(index)
                .ok_or(Error::Custom("invalid cell mask identifier"))?;
            let requested = request
                .cell_mask
                .mask_at(request_index)
                .ok_or(Error::Custom("invalid cell mask identifier"))?;
            let bits = BlobCellMask::new(mask).bits();
            if bits & !BlobCellMask::new(requested).bits() != 0 {
                return Err(Error::Custom("unrequested cell index"))
            }
            let count = bits.count_ones() as usize;
            if (count == 0 && !self.cells[index].is_empty()) ||
                (count != 0 &&
                    (self.cells[index].is_empty() ||
                        !self.cells[index].len().is_multiple_of(count)))
            {
                return Err(Error::Custom("invalid cell response dimensions"))
            }
        }
        Ok(())
    }
}

impl Encodable for Cells {
    fn encode(&self, out: &mut dyn BufMut) {
        self.hashes.encode(out);
        self.cells.encode(out);
        self.cell_mask.encode(out);
    }

    fn length(&self) -> usize {
        self.hashes.length() + self.cells.length() + self.cell_mask.length()
    }
}

impl Decodable for Cells {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let mut tail = *buf;
        Header::decode_bytes(&mut tail, true)?;
        let mut cell_lists = Header::decode_bytes(&mut tail, true)?;
        let cell_mask = CellMasks::decode(&mut tail)?;
        let hashes = if cell_mask.is_indexed() {
            decode_bounded_list(buf, MAX_INDEXED_CELL_HASHES)?
        } else {
            Vec::<B256>::decode(buf)?
        };
        // EIP-8408 requires all mask metadata to be validated before cell-response allocation.
        cell_mask.validate(&hashes, true)?;
        let mut cells = Vec::new();
        while !cell_lists.is_empty() {
            if cells.len() == hashes.len() {
                return Err(Error::Custom("invalid cell response dimensions"))
            }
            cells.push(Vec::<Cell>::decode(&mut cell_lists)?);
        }
        if cells.len() != hashes.len() {
            return Err(Error::Custom("invalid cell response dimensions"))
        }
        *buf = tail;
        Ok(Self { hashes, cells, cell_mask })
    }
}

fn decode_bounded_list<T: Decodable>(buf: &mut &[u8], max: usize) -> alloy_rlp::Result<Vec<T>> {
    let mut payload = Header::decode_bytes(buf, true)?;
    let mut values = Vec::new();
    while !payload.is_empty() {
        if values.len() == max {
            return Err(Error::Custom("invalid indexed cell counts"))
        }
        values.push(T::decode(&mut payload)?);
    }
    Ok(values)
}

/// Converts a mask to its wire cells bitmap, the inverse of [`BlobCellMask::new`].
pub fn encode_cell_mask(mask: BlobCellMask) -> B128 {
    B128::from(mask.bits().to_le_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        message::RequestPair, EthMessage, EthMessageID, EthNetworkPrimitives, EthVersion,
        ProtocolMessage,
    };
    use alloy_primitives::{hex, U256};
    use proptest::prelude::*;

    fn indexed(hashes: Vec<B256>, masks: Vec<B128>, ids: &[u8]) -> GetCells {
        GetCells {
            hashes,
            cell_mask: CellMasks::Indexed { masks, mask_ids: Bytes::copy_from_slice(ids) },
        }
    }

    fn request() -> GetCells {
        indexed(
            vec![B256::ZERO, B256::repeat_byte(1)],
            vec![B128::repeat_byte(1), B128::repeat_byte(2)],
            &[0, 1],
        )
    }

    #[test]
    fn shared_wire_vector_is_flat() {
        let pair = RequestPair { request_id: 1, message: GetCells::default() };
        let expected = hex!("d301c09000000000000000000000000000000000");
        assert_eq!(alloy_rlp::encode(&pair), expected);
        assert_eq!(alloy_rlp::decode_exact::<RequestPair<GetCells>>(&expected).unwrap(), pair);
        let response = RequestPair { request_id: 1, message: Cells::default() };
        let expected = hex!("d401c0c09000000000000000000000000000000000");
        assert_eq!(alloy_rlp::encode(&response), expected);
        assert_eq!(alloy_rlp::decode_exact::<RequestPair<Cells>>(&expected).unwrap(), response);
    }

    #[test]
    fn shared_cell_framing_by_version() {
        let messages = [
            EthMessage::<EthNetworkPrimitives>::GetCells(RequestPair {
                request_id: 1,
                message: GetCells::default(),
            }),
            EthMessage::Cells(RequestPair { request_id: 1, message: Cells::default() }),
        ];
        for message in messages {
            let protocol = ProtocolMessage::from(message);
            let canonical = alloy_rlp::encode(&protocol);
            for version in [EthVersion::Eth72, EthVersion::Eth73] {
                let mut buf = canonical.as_slice();
                assert_eq!(ProtocolMessage::decode_message(version, &mut buf).unwrap(), protocol);
                assert!(buf.is_empty());
            }
        }
    }

    #[test]
    fn cell_payload_is_bounded() {
        for (wire, extra_field) in [
            (
                hex!("14d301c09000000000000000000000000000000000").to_vec(),
                hex!("14d401c0900000000000000000000000000000000080").to_vec(),
            ),
            (
                hex!("15d401c0c09000000000000000000000000000000000").to_vec(),
                hex!("15d501c0c0900000000000000000000000000000000080").to_vec(),
            ),
        ] {
            for version in [EthVersion::Eth72, EthVersion::Eth73] {
                let mut appended = wire.clone();
                appended.push(0x80);
                let mut buf = appended.as_slice();
                ProtocolMessage::<EthNetworkPrimitives>::decode_message(version, &mut buf).unwrap();
                assert_eq!(buf, &[0x80]);
                assert!(matches!(
                    ProtocolMessage::<EthNetworkPrimitives>::decode_message(
                        version,
                        &mut extra_field.as_slice()
                    ),
                    Err(crate::message::MessageError::RlpError(Error::UnexpectedLength))
                ));
            }
        }
    }

    #[test]
    fn indexed_packets_match_geth_rlp_vectors() {
        let vectors: serde_json::Value =
            serde_json::from_str(include_str!("../testdata/eip8408/vectors.json",)).unwrap();
        let hashes = [0x11u64, 0x22, 0x33].map(|value| B256::from(U256::from(value))).to_vec();
        let request = indexed(
            hashes.clone(),
            vec![
                B128::from((1u128 | (1 << 8) | (1 << 127)).to_le_bytes()),
                B128::from((2u128 | (1 << 9)).to_le_bytes()),
            ],
            &[0, 1, 0],
        );
        let mut first = Cell::ZERO;
        first[0] = 1;
        first[2047] = 2;
        let mut second = Cell::ZERO;
        second[0] = 3;
        second[2047] = 4;
        let response = Cells {
            hashes: hashes[1..].to_vec(),
            cells: vec![vec![first], vec![second]],
            cell_mask: CellMasks::Indexed {
                masks: vec![
                    B128::from((1u128 << 9).to_le_bytes()),
                    B128::from((1u128 << 127).to_le_bytes()),
                ],
                mask_ids: Bytes::from_static(&[0, 1]),
            },
        };
        assert_eq!(response.validate_response(&request), Ok(()));
        let empty = Cells {
            cell_mask: CellMasks::Indexed { masks: vec![], mask_ids: Bytes::new() },
            ..Default::default()
        };
        for (name, message) in [
            (
                "request",
                EthMessage::<EthNetworkPrimitives>::GetCells(RequestPair {
                    request_id: 42,
                    message: request,
                }),
            ),
            ("response", EthMessage::Cells(RequestPair { request_id: 42, message: response })),
            ("empty_response", EthMessage::Cells(RequestPair { request_id: 42, message: empty })),
        ] {
            let expected = vectors[name].as_str().unwrap().parse::<Bytes>().unwrap();
            let protocol = ProtocolMessage::from(message);
            assert_eq!(alloy_rlp::encode(&protocol).as_slice(), expected.as_ref());
            let mut buf = expected.as_ref();
            assert_eq!(
                ProtocolMessage::decode_message(EthVersion::Eth73, &mut buf).unwrap(),
                protocol
            );
            assert!(buf.is_empty());
        }
    }

    #[test]
    fn indexed_wire_vector_is_flat() {
        // Local schema fixture; independent geth RLP vectors are checked above.
        let pair = RequestPair { request_id: 1, message: request() };
        let expected = hex!("f86b01f842a00000000000000000000000000000000000000000000000000000000000000000a00101010101010101010101010101010101010101010101010101010101010101e290010101010101010101010101010101019002020202020202020202020202020202820001");
        assert_eq!(alloy_rlp::encode(&pair), expected);
        assert_eq!(alloy_rlp::decode_exact::<RequestPair<GetCells>>(&expected).unwrap(), pair);
        assert_eq!(pair.length(), expected.len());
    }

    #[test]
    fn empty_indexed_response_vector() {
        let pair = RequestPair {
            request_id: 1,
            message: Cells {
                cell_mask: CellMasks::Indexed { masks: vec![], mask_ids: Bytes::new() },
                ..Default::default()
            },
        };
        let expected = hex!("c501c0c0c080");
        assert_eq!(alloy_rlp::encode(&pair), expected);
        assert_eq!(alloy_rlp::decode_exact::<RequestPair<Cells>>(&expected).unwrap(), pair);
        let empty = indexed(vec![], vec![], &[]);
        assert_eq!(
            GetCells::decode(&mut alloy_rlp::encode(empty).as_slice()),
            Err(Error::Custom("invalid indexed cell counts"))
        );
    }

    #[test]
    fn rejects_noncanonical_tables() {
        let hashes = request().hashes;
        let a = B128::repeat_byte(1);
        let b = B128::repeat_byte(2);
        let cases = [
            (indexed(hashes.clone(), vec![a, b], &[0]), "invalid indexed cell counts"),
            (indexed(hashes.clone(), vec![], &[0, 0]), "invalid indexed cell counts"),
            (indexed(hashes.clone(), vec![a, b], &[1, 0]), "invalid cell mask identifier order"),
            (indexed(hashes.clone(), vec![a, b], &[0, 2]), "invalid cell mask identifier order"),
            (indexed(hashes.clone(), vec![a, b], &[0, 255]), "invalid cell mask identifier order"),
            (indexed(hashes.clone(), vec![a, b], &[0, 0]), "unreferenced cell mask"),
            (indexed(hashes, vec![a, a], &[0, 1]), "duplicate cell mask"),
            (indexed(vec![B256::ZERO; 2], vec![a, b], &[0, 1]), "duplicate cell transaction hash"),
        ];
        for (request, error) in cases {
            assert_eq!(
                GetCells::decode(&mut alloy_rlp::encode(request).as_slice()),
                Err(Error::Custom(error))
            );
        }
    }

    #[test]
    fn indexed_bounds_and_shared_compatibility() {
        for count in [1, 2, 64, 65] {
            let hashes = (0..count).map(|n| B256::repeat_byte(n as u8)).collect::<Vec<_>>();
            let masks =
                if count == 1 { vec![B128::ZERO] } else { vec![B128::ZERO, B128::repeat_byte(1)] };
            let ids = (0..count).map(|index| (index % 2) as u8).collect::<Vec<_>>();
            let req = indexed(hashes.clone(), masks, &ids);
            let decoded = GetCells::decode(&mut alloy_rlp::encode(&req).as_slice());
            if count == 1 {
                assert_eq!(
                    decoded,
                    Err(Error::Custom("uniform cell requests must use the shared form"))
                );
            } else if count <= 64 {
                assert_eq!(decoded, Ok(req));
            } else {
                assert_eq!(decoded, Err(Error::Custom("invalid indexed cell counts")));
            }
            let shared = GetCells { hashes, cell_mask: B128::ZERO.into() };
            assert_eq!(GetCells::decode(&mut alloy_rlp::encode(&shared).as_slice()), Ok(shared));
        }
        let oversized = indexed(vec![B256::ZERO], vec![B128::ZERO; 65], &[0]);
        assert_eq!(
            GetCells::decode(&mut alloy_rlp::encode(oversized).as_slice()),
            Err(Error::Custom("invalid indexed cell counts"))
        );
        let oversized_ids = indexed(vec![B256::ZERO], vec![B128::ZERO], &[0; 65]);
        assert_eq!(
            GetCells::decode(&mut alloy_rlp::encode(oversized_ids).as_slice()),
            Err(Error::Custom("invalid indexed cell counts"))
        );
    }

    #[test]
    fn adaptive_encoding_keeps_uniform_requests_shared() {
        let hashes = vec![B256::ZERO, B256::repeat_byte(1), B256::repeat_byte(2)];
        let a = B128::repeat_byte(1);
        let b = B128::repeat_byte(2);
        assert_eq!(
            GetCells::from_masks(hashes.clone(), vec![a; 3]).unwrap().cell_mask,
            CellMasks::Shared(a)
        );
        assert_eq!(
            GetCells::from_masks(hashes, vec![a, b, a]).unwrap().cell_mask,
            CellMasks::Indexed { masks: vec![a, b], mask_ids: Bytes::from_static(&[0, 1, 0]) }
        );
        assert_eq!(
            GetCells::from_masks(vec![], vec![]),
            Err(Error::Custom("invalid indexed cell counts"))
        );
        assert_eq!(
            GetCells::from_masks(vec![B256::ZERO], vec![]),
            Err(Error::Custom("invalid indexed cell counts"))
        );
    }

    #[test]
    fn response_metadata_precedes_cell_decoding() {
        let req = indexed(vec![B256::ZERO], vec![B128::ZERO], &[1]);
        let mut wire = alloy_rlp::encode(&req.hashes);
        // The cell is intentionally malformed. Invalid metadata must win before cell decoding.
        wire.extend_from_slice(&[0xc1, 0x80]);
        req.cell_mask.encode(&mut wire);
        assert_eq!(
            Cells::decode(&mut wire.as_slice()),
            Err(Error::Custom("invalid cell mask identifier order"))
        );
    }

    #[test]
    fn decode_bounds_precede_element_decoding() {
        // Each list holds one malformed element past its bound, which must not be decoded.
        let mut payload = alloy_rlp::encode(B256::ZERO).repeat(MAX_INDEXED_CELL_HASHES);
        payload.push(0x80);
        let mut hashes = Vec::new();
        Header { list: true, payload_length: payload.len() }.encode(&mut hashes);
        hashes.extend_from_slice(&payload);
        let masks = CellMasks::Indexed {
            masks: vec![B128::ZERO, B128::repeat_byte(1)],
            mask_ids: (0..MAX_INDEXED_CELL_HASHES).map(|index| (index % 2) as u8).collect(),
        };
        let mut request = hashes.clone();
        masks.encode(&mut request);
        assert_eq!(
            GetCells::decode(&mut request.as_slice()),
            Err(Error::Custom("invalid indexed cell counts"))
        );
        let mut response = hashes;
        response.push(0xc0);
        masks.encode(&mut response);
        assert_eq!(
            Cells::decode(&mut response.as_slice()),
            Err(Error::Custom("invalid indexed cell counts"))
        );
        let mut response = alloy_rlp::encode(vec![B256::ZERO]);
        response.extend_from_slice(&hex!("c3c0c180"));
        B128::ZERO.encode(&mut response);
        assert_eq!(
            Cells::decode(&mut response.as_slice()),
            Err(Error::Custom("invalid cell response dimensions"))
        );
    }

    #[test]
    fn response_correlation_and_subsets() {
        let request = indexed(
            vec![B256::ZERO, B256::repeat_byte(1)],
            vec![B128::from(3u128.to_le_bytes()), B128::from(4u128.to_le_bytes())],
            &[0, 1],
        );
        let mut response = Cells {
            hashes: vec![B256::ZERO],
            cells: vec![vec![Cell::ZERO]],
            cell_mask: CellMasks::Indexed {
                masks: vec![B128::from(1u128.to_le_bytes())],
                mask_ids: Bytes::from_static(&[0]),
            },
        };
        assert_eq!(response.validate_response(&request), Ok(()));
        response.hashes[0] = B256::repeat_byte(2);
        assert_eq!(
            response.validate_response(&request),
            Err(Error::Custom("unrequested cell transaction hash"))
        );
        response.hashes[0] = B256::ZERO;
        response.cell_mask = CellMasks::Indexed {
            masks: vec![B128::from(4u128.to_le_bytes())],
            mask_ids: Bytes::from_static(&[0]),
        };
        assert_eq!(
            response.validate_response(&request),
            Err(Error::Custom("unrequested cell index"))
        );
        response.cell_mask = CellMasks::Shared(B128::ZERO);
        assert_eq!(
            response.validate_response(&request),
            Err(Error::Custom("invalid cell response form"))
        );
        response.cell_mask = CellMasks::Indexed {
            masks: vec![B128::from(3u128.to_le_bytes())],
            mask_ids: Bytes::from_static(&[0]),
        };
        assert_eq!(
            response.validate_response(&request),
            Err(Error::Custom("invalid cell response dimensions"))
        );
        response.cell_mask =
            CellMasks::Indexed { masks: vec![B128::ZERO], mask_ids: Bytes::from_static(&[0]) };
        assert_eq!(
            response.validate_response(&request),
            Err(Error::Custom("invalid cell response dimensions"))
        );
        response.cells.clear();
        assert_eq!(
            response.validate_response(&request),
            Err(Error::Custom("invalid cell response form"))
        );
    }

    #[test]
    fn shared_response_hash_lookup() {
        for count in [1, MAX_INDEXED_CELL_HASHES, MAX_INDEXED_CELL_HASHES + 1, 4096] {
            let request = GetCells {
                hashes: (0..count).map(|index| B256::from(U256::from(index))).collect(),
                cell_mask: B128::from(1u128.to_le_bytes()).into(),
            };
            let mut response = Cells {
                hashes: request.hashes.clone(),
                cells: vec![vec![]; count],
                cell_mask: B128::ZERO.into(),
            };
            assert_eq!(response.validate_response(&request), Ok(()));
            if count > 1 {
                let mut repeated = response.clone();
                repeated.hashes.fill(request.hashes[count - 1]);
                assert_eq!(
                    repeated.validate_response(&request),
                    Err(Error::Custom("duplicate cell transaction hash"))
                );
            }
            response.hashes[count - 1] = B256::repeat_byte(0xff);
            assert_eq!(
                response.validate_response(&request),
                Err(Error::Custom("unrequested cell transaction hash"))
            );
            response.hashes[count - 1] = request.hashes[count - 1];
            response.cell_mask = B128::from(2u128.to_le_bytes()).into();
            assert_eq!(
                response.validate_response(&request),
                Err(Error::Custom("unrequested cell index"))
            );
        }
    }

    #[test]
    fn small_shared_response_to_large_request() {
        let request = GetCells {
            hashes: (0..=MAX_INDEXED_CELL_HASHES)
                .map(|index| B256::from(U256::from(index)))
                .collect(),
            cell_mask: B128::ZERO.into(),
        };
        for count in [0, 1, MAX_INDEXED_CELL_HASHES] {
            let mut response = Cells {
                hashes: request.hashes.iter().rev().take(count).copied().collect(),
                cells: vec![vec![]; count],
                cell_mask: B128::ZERO.into(),
            };
            assert_eq!(response.validate_response(&request), Ok(()));
            if let Some(hash) = response.hashes.last_mut() {
                *hash = B256::repeat_byte(0xff);
                assert_eq!(
                    response.validate_response(&request),
                    Err(Error::Custom("unrequested cell transaction hash"))
                );
            }
        }
    }

    #[test]
    fn shared_response_respects_requested_hash_multiplicity() {
        for count in [3, MAX_INDEXED_CELL_HASHES, MAX_INDEXED_CELL_HASHES + 1, 4096] {
            let mut hashes =
                (0..count).map(|index| B256::from(U256::from(index))).collect::<Vec<_>>();
            hashes[count - 1] = hashes[0];
            let request = GetCells { hashes: hashes.clone(), cell_mask: B128::ZERO.into() };
            let mut response =
                Cells { hashes, cells: vec![vec![]; count], cell_mask: B128::ZERO.into() };
            assert_eq!(response.validate_response(&request), Ok(()));
            response.hashes[1] = response.hashes[0];
            assert_eq!(
                response.validate_response(&request),
                Err(Error::Custom("duplicate cell transaction hash"))
            );
        }
    }

    #[test]
    fn uniform_indexed_requests_are_rejected_but_responses_are_valid() {
        let hashes = vec![B256::ZERO, B256::repeat_byte(1)];
        let a = B128::from(1u128.to_le_bytes());
        let b = B128::from(2u128.to_le_bytes());
        let uniform = indexed(hashes.clone(), vec![a], &[0, 0]);
        assert_eq!(
            GetCells::decode(&mut alloy_rlp::encode(uniform).as_slice()),
            Err(Error::Custom("uniform cell requests must use the shared form"))
        );
        let request = GetCells::from_masks(hashes.clone(), vec![a, b]).unwrap();
        let response = Cells {
            hashes: vec![hashes[0]],
            cells: vec![vec![Cell::ZERO]],
            cell_mask: CellMasks::Indexed { masks: vec![a], mask_ids: Bytes::from_static(&[0]) },
        };
        assert_eq!(response.validate_response(&request), Ok(()));
        assert_eq!(
            alloy_rlp::decode_exact::<RequestPair<Cells>>(&alloy_rlp::encode(RequestPair {
                request_id: 1,
                message: response.clone(),
            }))
            .unwrap()
            .message,
            response
        );
    }

    #[test]
    fn decoding_is_bounded_by_request_pair() {
        let pair = RequestPair { request_id: 1, message: request() };
        let mut wire = alloy_rlp::encode(&pair);
        wire.push(0x80);
        let mut buf = wire.as_slice();
        assert_eq!(RequestPair::<GetCells>::decode(&mut buf).unwrap(), pair);
        assert_eq!(buf, &[0x80]);
        assert_eq!(
            RequestPair::<GetCells>::decode(
                &mut &hex!("c101c09000000000000000000000000000000000")[..]
            ),
            Err(Error::InputTooShort)
        );
    }

    #[test]
    fn indexed_forms_require_eth73() {
        let messages = [
            EthMessage::<EthNetworkPrimitives>::GetCells(RequestPair {
                request_id: 1,
                message: request(),
            }),
            EthMessage::Cells(RequestPair {
                request_id: 1,
                message: Cells {
                    cell_mask: CellMasks::Indexed { masks: vec![], mask_ids: Bytes::new() },
                    ..Default::default()
                },
            }),
        ];
        for message in messages {
            let message_type = message.message_id();
            let protocol = ProtocolMessage { message_type, message };
            let encoded = alloy_rlp::encode(&protocol);
            assert_eq!(
                ProtocolMessage::<EthNetworkPrimitives>::decode_message(
                    EthVersion::Eth73,
                    &mut encoded.as_slice()
                )
                .unwrap(),
                protocol
            );
            assert!(
                matches!(ProtocolMessage::<EthNetworkPrimitives>::decode_message(EthVersion::Eth72,&mut encoded.as_slice()),Err(crate::message::MessageError::Invalid(EthVersion::Eth72, id)) if id == message_type)
            );
        }
        assert_eq!(
            EthMessageID::message_count(EthVersion::Eth73),
            EthMessageID::message_count(EthVersion::Eth72)
        );
    }

    #[test]
    fn indexed_response_roundtrip_and_dimensions() {
        let request = request();
        let response = Cells {
            hashes: request.hashes.clone(),
            cells: vec![vec![Cell::ZERO; 16], vec![Cell::repeat_byte(1); 16]],
            cell_mask: request.cell_mask.clone(),
        };
        assert_eq!(response.validate_response(&request), Ok(()));
        let pair = RequestPair { request_id: 9, message: response.clone() };
        let wire = alloy_rlp::encode(&pair);
        assert_eq!(pair.length(), wire.len());
        assert_eq!(alloy_rlp::decode_exact::<RequestPair<Cells>>(&wire).unwrap(), pair);
        let mut missing = response.clone();
        missing.cells.pop();
        assert_eq!(
            Cells::decode(&mut alloy_rlp::encode(missing).as_slice()),
            Err(Error::Custom("invalid cell response dimensions"))
        );
        let mut extra = response.clone();
        extra.cells.push(vec![]);
        assert_eq!(
            Cells::decode(&mut alloy_rlp::encode(extra).as_slice()),
            Err(Error::Custom("invalid cell response dimensions"))
        );
        let mut no_cells = response;
        no_cells.cells[0].clear();
        assert_eq!(
            no_cells.validate_response(&request),
            Err(Error::Custom("invalid cell response dimensions"))
        );
    }

    #[test]
    fn adaptive_rejects_invalid_heterogeneous_batches() {
        let hashes = (0..65).map(B256::repeat_byte).collect::<Vec<_>>();
        let masks = (0..65).map(B128::repeat_byte).collect::<Vec<_>>();
        assert_eq!(
            GetCells::from_masks(hashes, masks),
            Err(Error::Custom("invalid indexed cell counts"))
        );
        assert_eq!(
            GetCells::from_masks(vec![B256::ZERO; 2], vec![B128::ZERO, B128::repeat_byte(1)]),
            Err(Error::Custom("duplicate cell transaction hash"))
        );
        let hashes = (0..64).map(B256::repeat_byte).collect::<Vec<_>>();
        let masks = (0..64).map(B128::repeat_byte).collect::<Vec<_>>();
        let request = GetCells::from_masks(hashes, masks).unwrap();
        assert_eq!(request.cell_mask.validate(&request.hashes, false), Ok(()));
    }

    proptest! {
        #[test]
        fn adaptive_roundtrip(mask_values in proptest::collection::vec(any::<u128>(),1..=64)) {
            let hashes = (0..mask_values.len()).map(|i| B256::repeat_byte(i as u8)).collect();
            let masks = mask_values.iter().map(|n| B128::from(n.to_le_bytes())).collect();
            let req = GetCells::from_masks(hashes,masks).unwrap();
            let pair = RequestPair { request_id: 1,message: req };
            let wire = alloy_rlp::encode(&pair);
            prop_assert_eq!(pair.length(),wire.len());
            prop_assert_eq!(alloy_rlp::decode_exact::<RequestPair<GetCells>>(&wire).unwrap(),pair);
        }

        #[test]
        fn shared_versions_roundtrip(
            hashes in proptest::collection::vec(any::<[u8; 32]>(), 0..70),
            mask in any::<[u8; 16]>(),
            request_id in any::<u64>(),
        ) {
            let hashes = hashes.into_iter().map(B256::from).collect::<Vec<_>>();
            let messages = [
                EthMessage::<EthNetworkPrimitives>::GetCells(RequestPair {
                    request_id,
                    message: GetCells { hashes: hashes.clone(), cell_mask: B128::from(mask).into() },
                }),
                EthMessage::Cells(RequestPair {
                    request_id,
                    message: Cells {
                        cells: vec![vec![]; hashes.len()],
                        hashes,
                        cell_mask: B128::ZERO.into(),
                    },
                }),
            ];
            for message in messages {
                let protocol = ProtocolMessage::from(message);
                for version in [EthVersion::Eth72, EthVersion::Eth73] {
                    let wire = alloy_rlp::encode(&protocol);
                    let mut buf = wire.as_slice();
                    prop_assert_eq!(&ProtocolMessage::decode_message(version, &mut buf).unwrap(),
                        &protocol);
                    prop_assert!(buf.is_empty());
                }
            }
        }

        #[test]
        fn arbitrary_input_does_not_panic(data in proptest::collection::vec(any::<u8>(),0..4096)) {
            let _request = RequestPair::<GetCells>::decode(&mut data.as_slice());
            let _response = RequestPair::<Cells>::decode(&mut data.as_slice());
            for version in [EthVersion::Eth72, EthVersion::Eth73] {
                let _message = ProtocolMessage::<EthNetworkPrimitives>::decode_message(
                    version, &mut data.as_slice());
            }
        }
    }
}

#[cfg(all(test, feature = "serde"))]
mod serde_tests {
    use super::*;

    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct LegacyGetCells {
        hashes: Vec<B256>,
        cell_mask: B128,
    }

    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct LegacyCells {
        hashes: Vec<B256>,
        cells: Vec<Vec<Cell>>,
        cell_mask: B128,
    }

    #[test]
    fn shared_requests_preserve_legacy_serde() {
        for mask in [B128::ZERO, B128::repeat_byte(0x81)] {
            let legacy = LegacyGetCells { hashes: vec![B256::ZERO], cell_mask: mask };
            let request = GetCells { hashes: legacy.hashes.clone(), cell_mask: mask.into() };
            let json = serde_json::to_string(&legacy).unwrap();
            assert_eq!(serde_json::to_string(&request).unwrap(), json);
            assert_eq!(serde_json::from_str::<GetCells>(&json).unwrap(), request);
            assert_eq!(serde_json::from_str::<LegacyGetCells>(&json).unwrap(), legacy);
            let bytes = bincode::serialize(&legacy).unwrap();
            assert_eq!(bincode::serialize(&request).unwrap(), bytes);
            assert_eq!(bincode::deserialize::<GetCells>(&bytes).unwrap(), request);
            assert_eq!(bincode::deserialize::<LegacyGetCells>(&bytes).unwrap(), legacy);
        }
    }

    #[test]
    fn shared_responses_preserve_legacy_serde() {
        for mask in [B128::ZERO, B128::repeat_byte(0x81)] {
            let legacy = LegacyCells {
                hashes: vec![B256::ZERO],
                cells: vec![vec![
                    Cell::ZERO;
                    u128::from_le_bytes(mask.into()).count_ones() as usize
                ]],
                cell_mask: mask,
            };
            let response = Cells {
                hashes: legacy.hashes.clone(),
                cells: legacy.cells.clone(),
                cell_mask: mask.into(),
            };
            let json = serde_json::to_string(&legacy).unwrap();
            assert_eq!(serde_json::to_string(&response).unwrap(), json);
            assert_eq!(serde_json::from_str::<Cells>(&json).unwrap(), response);
            assert_eq!(serde_json::from_str::<LegacyCells>(&json).unwrap(), legacy);
            let bytes = bincode::serialize(&legacy).unwrap();
            assert_eq!(bincode::serialize(&response).unwrap(), bytes);
            assert_eq!(bincode::deserialize::<Cells>(&bytes).unwrap(), response);
            assert_eq!(bincode::deserialize::<LegacyCells>(&bytes).unwrap(), legacy);
        }
    }

    #[test]
    fn indexed_masks_serde_roundtrip() {
        for count in [0, 1, MAX_INDEXED_CELL_MASKS] {
            let masks = CellMasks::Indexed {
                masks: (0..count).map(|index| B128::repeat_byte(index as u8)).collect(),
                mask_ids: (0..count).map(|index| index as u8).collect::<Vec<_>>().into(),
            };
            let json = serde_json::to_string(&masks).unwrap();
            assert_eq!(serde_json::from_str::<CellMasks>(&json).unwrap(), masks);
            let bytes = bincode::serialize(&masks).unwrap();
            assert_eq!(bincode::deserialize::<CellMasks>(&bytes).unwrap(), masks);
        }
        assert_eq!(
            serde_json::to_string(&CellMasks::Indexed { masks: vec![], mask_ids: Bytes::new() })
                .unwrap(),
            "\"0xc080\""
        );
    }

    #[test]
    fn serde_rejects_ambiguous_and_trailing_encodings() {
        // This malformed indexed table would otherwise collide with a raw B128's length.
        let ambiguous = CellMasks::Indexed { masks: vec![], mask_ids: vec![0; 14].into() };
        assert_eq!(alloy_rlp::encode(&ambiguous).len(), B128::len_bytes());
        assert!(matches!(bincode::serialize(&ambiguous).unwrap_err().as_ref(),
            bincode::ErrorKind::Custom(message) if message == "invalid indexed cell mask encoding"));
        for masks in [
            CellMasks::Shared(B128::ZERO),
            CellMasks::Indexed { masks: vec![], mask_ids: Bytes::new() },
        ] {
            let mut encoded = alloy_rlp::encode(&masks);
            if masks.is_indexed() {
                encoded.push(0x80);
            }
            let bytes = bincode::serialize(&Bytes::from(encoded)).unwrap();
            assert!(matches!(bincode::deserialize::<CellMasks>(&bytes).unwrap_err().as_ref(),
                bincode::ErrorKind::Custom(message) if message == "invalid indexed cell mask encoding"));
        }
    }

    #[test]
    fn serde_rejects_malformed_or_oversized_indexed_bytes() {
        for (encoded, expected) in [
            (Bytes::new(), Error::InputTooShort),
            (
                Bytes::from(alloy_rlp::encode(CellMasks::Indexed {
                    masks: vec![B128::ZERO; MAX_INDEXED_CELL_MASKS + 1],
                    mask_ids: Bytes::new(),
                })),
                Error::Custom("invalid indexed cell counts"),
            ),
            (
                Bytes::from(alloy_rlp::encode(CellMasks::Indexed {
                    masks: vec![],
                    mask_ids: vec![0; MAX_INDEXED_CELL_HASHES + 1].into(),
                })),
                Error::Custom("invalid indexed cell counts"),
            ),
        ] {
            let bytes = bincode::serialize(&encoded).unwrap();
            assert!(matches!(bincode::deserialize::<CellMasks>(&bytes).unwrap_err().as_ref(),
                bincode::ErrorKind::Custom(message) if *message == expected.to_string()));
        }
    }

    #[test]
    fn serde_bounds_input_before_hex_decoding() {
        let encoded = Bytes::from(vec![0; MAX_SERIALIZED_CELL_MASK_BYTES + 1]);
        let value = serde_json::to_value(&encoded).unwrap();
        assert_eq!(
            serde_json::from_value::<CellMasks>(value).unwrap_err().to_string(),
            "serialized cell masks exceed indexed bounds"
        );
        let bytes = bincode::serialize(&encoded).unwrap();
        assert!(matches!(bincode::deserialize::<CellMasks>(&bytes).unwrap_err().as_ref(),
            bincode::ErrorKind::Custom(message) if message == "serialized cell masks exceed indexed bounds"));
        for masks in [
            CellMasks::Indexed {
                masks: vec![B128::ZERO; MAX_INDEXED_CELL_MASKS + 1],
                mask_ids: Bytes::new(),
            },
            CellMasks::Indexed {
                masks: vec![],
                mask_ids: vec![0; MAX_INDEXED_CELL_HASHES + 1].into(),
            },
        ] {
            assert!(matches!(bincode::serialize(&masks).unwrap_err().as_ref(),
                bincode::ErrorKind::Custom(message) if message == "invalid indexed cell counts"));
        }
    }

    #[test]
    fn indexed_responses_serde_roundtrip() {
        for response in [
            Cells {
                cell_mask: CellMasks::Indexed { masks: vec![], mask_ids: Bytes::new() },
                ..Default::default()
            },
            Cells {
                hashes: vec![B256::ZERO],
                cells: vec![vec![Cell::ZERO]],
                cell_mask: CellMasks::Indexed {
                    masks: vec![B128::from(1u128.to_le_bytes())],
                    mask_ids: vec![0].into(),
                },
            },
        ] {
            let json = serde_json::to_string(&response).unwrap();
            assert_eq!(serde_json::from_str::<Cells>(&json).unwrap(), response);
            let bytes = bincode::serialize(&response).unwrap();
            assert_eq!(bincode::deserialize::<Cells>(&bytes).unwrap(), response);
        }
        assert_eq!(
            serde_json::from_str::<CellMasks>(&serde_json::to_string(&[0u8; 16]).unwrap()).unwrap(),
            CellMasks::Shared(B128::ZERO)
        );
    }

    proptest::proptest! {
        #[test]
        fn adaptive_masks_serde_roundtrip(values in proptest::collection::vec(proptest::prelude::any::<u128>(), 1..=64)) {
            let hashes = (0..values.len()).map(|index| B256::repeat_byte(index as u8)).collect();
            let masks = values.into_iter().map(|bits| B128::from(bits.to_le_bytes())).collect();
            let request = GetCells::from_masks(hashes, masks).unwrap();
            let json = serde_json::to_string(&request).unwrap();
            proptest::prop_assert_eq!(&serde_json::from_str::<GetCells>(&json).unwrap(), &request);
            let bytes = bincode::serialize(&request).unwrap();
            proptest::prop_assert_eq!(bincode::deserialize::<GetCells>(&bytes).unwrap(), request);
        }
    }
}
