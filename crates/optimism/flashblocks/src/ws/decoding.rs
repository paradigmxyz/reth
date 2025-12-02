use alloy_primitives::bytes::Bytes;
use std::io;

/// A trait for decoding flashblocks from bytes into payload type `F`.
pub trait FlashBlockDecoder<F>: Send + 'static {
    /// Decodes `bytes` into a flashblock payload of type `F`.
    fn decode(&self, bytes: Bytes) -> eyre::Result<F>;
}

impl<F> FlashBlockDecoder<F> for ()
where
    F: serde::de::DeserializeOwned,
{
    fn decode(&self, bytes: Bytes) -> eyre::Result<F> {
        decode_flashblock(bytes)
    }
}

fn decode_flashblock<F>(bytes: Bytes) -> eyre::Result<F>
where
    F: serde::de::DeserializeOwned,
{
    let bytes = try_decompress(bytes)?;

    let payload: F =
        serde_json::from_slice(&bytes).map_err(|e| eyre::eyre!("failed to parse message: {e}"))?;

    Ok(payload)
}

/// Maps `bytes` into a potentially different [`Bytes`].
///
/// If the bytes start with a "{" character, prepended by any number of ASCII-whitespaces,
/// then it assumes that it is JSON-encoded and returns it as-is.
///
/// Otherwise, the `bytes` are passed through a brotli decompressor and returned.
fn try_decompress(bytes: Bytes) -> eyre::Result<Bytes> {
    if bytes.trim_ascii_start().starts_with(b"{") {
        return Ok(bytes);
    }

    let mut decompressor = brotli::Decompressor::new(bytes.as_ref(), 4096);
    let mut decompressed = Vec::new();
    io::copy(&mut decompressor, &mut decompressed)?;

    Ok(decompressed.into())
}
