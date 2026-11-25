//! Shared (EIP-8070) and indexed (EIP-8408) cell retrieval messages.
//!
//! Payload codecs are flattened: `RequestPair` supplies the outer RLP list and request id.

use alloc::vec::Vec;
use alloy_eips::eip7594::{BlobCellMask, Cell};
use alloy_primitives::{Bytes, B128, B256};
use alloy_rlp::{BufMut, Decodable, Encodable, Error, Header};
use reth_primitives_traits::InMemorySize;

/// Mandatory transaction count bound for indexed cell messages (EIP-8408).
pub const MAX_INDEXED_CELL_HASHES: usize = 64;

/// Mandatory mask table bound for indexed cell messages (EIP-8408).
pub const MAX_INDEXED_CELL_MASKS: usize = 64;

/// Cell indices shared by all transactions or selected through a canonical mask table.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CellMasks {
    /// The eth/72 form, also accepted on eth/73 without indexed-form limits.
    Shared(B128),
    /// The eth/73 form. Masks must be unique, referenced, and ordered by first use.
    Indexed {
        /// One to 64 masks, or empty for an empty indexed response.
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
    /// Shared forms retain eth/72 semantics. Only responses may use an empty indexed table.
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
        self.cell_mask
            .mask_at(index)
            .map(|mask| BlobCellMask::from_bits(u128::from_le_bytes(mask.into())))
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
    /// Checks form, requested hashes, mask subsets, and cell-list dimensions.
    /// Cryptographic verification requires the transaction's commitments and proofs and is
    /// performed by the caller before importing any data into the blobpool.
    pub fn validate_response(&self, request: &GetCells) -> alloy_rlp::Result<()> {
        self.cell_mask.validate(&self.hashes, true)?;
        if self.cell_mask.is_indexed() != request.cell_mask.is_indexed() ||
            self.hashes.len() != self.cells.len()
        {
            return Err(Error::Custom("invalid cell response form"))
        }
        for (index, hash) in self.hashes.iter().enumerate() {
            let Some(request_index) = request.hashes.iter().position(|requested| requested == hash)
            else {
                return Err(Error::Custom("unrequested cell transaction hash"))
            };
            let mask = self
                .cell_mask
                .mask_at(index)
                .ok_or(Error::Custom("invalid cell mask identifier"))?;
            let requested = request
                .cell_mask
                .mask_at(request_index)
                .ok_or(Error::Custom("invalid cell mask identifier"))?;
            let bits = u128::from_le_bytes(mask.into());
            if bits & !u128::from_le_bytes(requested.into()) != 0 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        message::RequestPair, EthMessage, EthMessageID, EthNetworkPrimitives, EthVersion,
        ProtocolMessage,
    };
    use alloy_primitives::hex;
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
    fn indexed_wire_vector_is_flat() {
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
        for count in [1, 64, 65] {
            let hashes = (0..count).map(|n| B256::repeat_byte(n as u8)).collect::<Vec<_>>();
            let req = indexed(hashes.clone(), vec![B128::ZERO], &vec![0; count]);
            let decoded = GetCells::decode(&mut alloy_rlp::encode(&req).as_slice());
            if count <= 64 {
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
    fn response_correlation_and_subsets() {
        let request = indexed(vec![B256::ZERO], vec![B128::from(3u128.to_le_bytes())], &[0]);
        let mut response = Cells {
            hashes: vec![B256::ZERO],
            cells: vec![vec![Cell::ZERO]],
            cell_mask: CellMasks::Indexed {
                masks: vec![B128::from(1u128.to_le_bytes())],
                mask_ids: Bytes::from_static(&[0]),
            },
        };
        assert_eq!(response.validate_response(&request), Ok(()));
        response.hashes[0] = B256::repeat_byte(1);
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
        response.cells.clear();
        assert_eq!(
            response.validate_response(&request),
            Err(Error::Custom("invalid cell response form"))
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
        fn arbitrary_input_does_not_panic(data in proptest::collection::vec(any::<u8>(),0..4096)) {
            let _request = RequestPair::<GetCells>::decode(&mut data.as_slice());
            let _response = RequestPair::<Cells>::decode(&mut data.as_slice());
        }
    }
}
