//! Transaction-backed sparse reads and authenticated full-code reconstruction.

use crate::{tables, transaction::DbTx};
use alloy_primitives::{keccak256, Address, Bytes, B256};
use reth_codecs::{Compress, Decompress, DecompressError};
use reth_db_models::code_chunks::{
    CodeChunkDescriptor, CodeRepresentation, LEGACY_CODE_CHUNK_SIZE,
};
use reth_primitives_traits::Bytecode;
use reth_storage_errors::provider::{CodeChunkError, CodeChunkErrorKind, ProviderResult};

/// Read only one payload, using metadata for representation discovery.
/// Unknown hashes return absence; a known descriptor makes its payload required.
pub fn get_code_chunk_by_hash<T: DbTx>(
    tx: &T,
    hash: &B256,
    index: u32,
) -> ProviderResult<Option<Bytes>> {
    if *hash == keccak256([]) {
        return Ok(None);
    }
    if let Some(descriptor) = descriptor(tx, hash, index)? {
        return chunk(tx, hash, &descriptor, index);
    }
    legacy(tx, hash, index, false)
}

/// Read using the representation committed by the requested account view.
/// Empty and invalid-index requests do not touch storage.
pub fn get_required_code_chunk<T: DbTx>(
    tx: &T,
    hash: &B256,
    representation: &CodeRepresentation,
    index: u32,
) -> ProviderResult<Option<Bytes>> {
    match representation {
        CodeRepresentation::Empty => Ok(None),
        CodeRepresentation::Legacy => legacy(tx, hash, index, true),
        CodeRepresentation::Chunked(committed) => {
            let Some(range) = committed.chunk_range(index) else { return Ok(None) };
            let stored = descriptor(tx, hash, index)?.ok_or_else(|| {
                failure(hash, index, Some(range.len()), CodeChunkErrorKind::MissingDescriptor)
            })?;
            if !stored.same_commitment(committed) {
                return Err(failure(
                    hash,
                    index,
                    Some(range.len()),
                    CodeChunkErrorKind::DescriptorMismatch,
                )
                .into());
            }
            chunk(tx, hash, &stored, index)
        }
    }
}

/// Reconstruct original code, authenticating each commitment and the full identity.
/// Legacy rows retain their existing full-code semantics, including oversized rows.
pub fn bytecode_by_hash<T: DbTx>(tx: &T, hash: &B256) -> ProviderResult<Option<Bytecode>> {
    let Some(metadata) = descriptor(tx, hash, 0)? else {
        return tx.get_by_encoded_key::<tables::Bytecodes>(hash).map_err(Into::into);
    };
    let mut original = Vec::with_capacity(metadata.code_size() as usize);
    for (index, expected_hash) in metadata.chunk_hashes().iter().enumerate() {
        let bytes =
            chunk(tx, hash, &metadata, index as u32)?.expect("descriptor index is in range");
        if keccak256(&bytes) != *expected_hash {
            return Err(failure(
                hash,
                index as u32,
                Some(bytes.len()),
                CodeChunkErrorKind::ChunkHashMismatch,
            )
            .into());
        }
        original.extend_from_slice(&bytes);
    }
    if original.len() != metadata.code_size() as usize || keccak256(&original) != *hash {
        return Err(failure(
            hash,
            0,
            Some(metadata.chunk_range(0).expect("multi-chunk descriptor").len()),
            CodeChunkErrorKind::FullCodeHashMismatch,
        )
        .into());
    }
    let code = reth_db_models::code_chunks::ValidatedCode::new(original.into())
        .map_err(reth_storage_errors::provider::ProviderError::InvalidChunkedCode)?;
    if code.descriptor() != Some(&metadata) {
        return Err(failure(hash, 0, None, CodeChunkErrorKind::DescriptorMismatch).into());
    }
    Ok(Some(Bytecode(revm_bytecode::Bytecode::new_legacy(code.original_bytes().clone()))))
}

/// Load bounded descriptor and preparation data without reading a payload.
pub fn descriptor<T: DbTx>(
    tx: &T,
    hash: &B256,
    index: u32,
) -> ProviderResult<Option<CodeChunkDescriptor>> {
    tx.get_by_encoded_key::<tables::BytecodeChunkDescriptors>(hash).map_err(|error| {
        let reason = if matches!(error, crate::DatabaseError::Decode) {
            CodeChunkErrorKind::MalformedDescriptor
        } else {
            CodeChunkErrorKind::Database(error)
        };
        failure(hash, index, None, reason).into()
    })
}

fn chunk<T: DbTx>(
    tx: &T,
    hash: &B256,
    metadata: &CodeChunkDescriptor,
    index: u32,
) -> ProviderResult<Option<Bytes>> {
    let Some(range) = metadata.chunk_range(index) else { return Ok(None) };
    let expected = Some(range.len());
    let start = std::time::Instant::now();
    let result =
        tx.get_by_encoded_key::<tables::BytecodeChunks>(&metadata.chunk_hashes()[index as usize]);
    metrics::histogram!("provider.code_chunks.read_latency").record(start.elapsed().as_secs_f64());
    if let Ok(Some(bytes)) = &result {
        metrics::counter!("provider.code_chunks.bytes_fetched").increment(bytes.len() as u64);
    }
    let bytes = result
        .map_err(|error| failure(hash, index, expected, CodeChunkErrorKind::Database(error)))?
        .ok_or_else(|| failure(hash, index, expected, CodeChunkErrorKind::MissingPayload))?;
    if bytes.len() != range.len() {
        return Err(failure(
            hash,
            index,
            expected,
            CodeChunkErrorKind::InvalidLength { actual: bytes.len() },
        )
        .into());
    }
    // Ingestion authenticates immutable payloads. Sparse execution reads need no rehash.
    Ok(Some(bytes))
}

fn legacy<T: DbTx>(
    tx: &T,
    hash: &B256,
    index: u32,
    required: bool,
) -> ProviderResult<Option<Bytes>> {
    if index != 0 || *hash == keccak256([]) {
        return Ok(None);
    }
    let code = tx
        .get_by_encoded_key::<tables::Bytecodes>(hash)
        .map_err(|error| failure(hash, index, None, CodeChunkErrorKind::Database(error)))?;
    let Some(code) = code else {
        return if required {
            Err(failure(hash, index, None, CodeChunkErrorKind::MissingLegacyCode).into())
        } else {
            Ok(None)
        };
    };
    let bytes = code.original_bytes();
    if bytes.len() > LEGACY_CODE_CHUNK_SIZE {
        return Err(failure(
            hash,
            index,
            None,
            CodeChunkErrorKind::UnsupportedLegacySize { actual: bytes.len() },
        )
        .into());
    }
    Ok(Some(bytes))
}

fn failure(
    hash: &B256,
    index: u32,
    expected_length: Option<usize>,
    reason: CodeChunkErrorKind,
) -> CodeChunkError {
    CodeChunkError { code_hash: *hash, index, expected_length, reason, context: None }
}

/// Inspect only Compact framing and its code-kind tag, without copying runtime payloads.
/// This compatibility read applies to old account records, never inline delegation accounts.
pub fn legacy_delegation<T: DbTx>(tx: &T, hash: &B256) -> ProviderResult<Option<Address>> {
    if *hash == keccak256([]) {
        return Ok(None);
    }
    Ok(tx.get_by_encoded_key::<LegacyBytecodeHeader>(hash)?.and_then(|header| header.0))
}

#[derive(Debug)]
struct LegacyBytecodeHeader;

impl crate::table::Table for LegacyBytecodeHeader {
    const NAME: &'static str = "Bytecodes";
    const DUPSORT: bool = false;
    type Key = B256;
    type Value = LegacyDelegation;
}

#[derive(Debug, serde::Serialize)]
struct LegacyDelegation(Option<Address>);

impl Decompress for LegacyDelegation {
    fn decompress(value: &[u8]) -> Result<Self, DecompressError> {
        let invalid = || {
            DecompressError::new(
                reth_db_models::code_chunks::CodeValidationError::DescriptorEncoding,
            )
        };
        let length = u32::from_be_bytes(
            value.get(..4).ok_or_else(invalid)?.try_into().map_err(|_| invalid())?,
        ) as usize;
        let kind_offset = length.checked_add(4).ok_or_else(invalid)?;
        let kind = *value.get(kind_offset).ok_or_else(invalid)?;
        match kind {
            2 => {
                let original = u64::from_be_bytes(
                    value
                        .get(kind_offset + 1..kind_offset + 9)
                        .ok_or_else(invalid)?
                        .try_into()
                        .map_err(|_| invalid())?,
                );
                if original > length as u64 ||
                    value.len() - (kind_offset + 9) < (original as usize).div_ceil(8)
                {
                    return Err(invalid());
                }
                Ok(Self(None))
            }
            0 | 4 => {
                if value.len() != kind_offset + 1 {
                    return Err(invalid());
                }
                let marker = value.get(4..kind_offset).ok_or_else(invalid)?;
                if kind == 4 || marker.starts_with(&[0xef, 0x01]) {
                    if marker.len() != 23 || marker[..3] != [0xef, 0x01, 0x00] {
                        return Err(invalid());
                    }
                    Ok(Self(Some(Address::from_slice(&marker[3..]))))
                } else {
                    Ok(Self(None))
                }
            }
            _ => Err(invalid()),
        }
    }
}

impl Compress for LegacyDelegation {
    type Compressed = Vec<u8>;
    fn compress(self) -> Self::Compressed {
        panic!("read-only bytecode header view")
    }
    fn compress_to_buf<B: bytes::BufMut + AsMut<[u8]>>(&self, _: &mut B) {
        panic!("read-only bytecode header view")
    }
}

/// Return the stored legacy kind without copying or analyzing runtime payloads.
pub fn legacy_code_kind<T: DbTx>(tx: &T, hash: &B256) -> ProviderResult<Option<bool>> {
    if *hash == keccak256([]) {
        return Ok(Some(false))
    }
    Ok(tx.get_by_encoded_key::<LegacyBytecodeHeader>(hash)?.map(|header| header.0.is_some()))
}
