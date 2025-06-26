use super::FromRecoveredTx;
use crate::ScrollTransactionIntoTxEnv;
use alloy_consensus::transaction::Recovered;
use alloy_eips::{Encodable2718, Typed2718};
use alloy_evm::{IntoTxEnv, RecoveredTx};
use alloy_primitives::{Address, Bytes, TxKind, U256};
use revm::context::TxEnv;
use revm_scroll::l1block::TX_L1_FEE_PRECISION_U256;
use scroll_alloy_consensus::{ScrollTxEnvelope, TxL1Message};
pub use zstd_compression::compute_compression_ratio;

#[cfg(feature = "zstd_compression")]
mod zstd_compression {
    use super::*;
    use std::io::Write;
    use zstd::{
        stream::Encoder,
        zstd_safe::{CParameter, ParamSwitch},
    };

    /// The maximum size of the compression window in bytes (`2^CL_WINDOW_LIMIT`).
    const CL_WINDOW_LIMIT: u32 = 22;

    /// The zstd block size target.
    const N_BLOCK_SIZE_TARGET: u32 = 124 * 1024;

    fn compressor(target_block_size: u32) -> Encoder<'static, Vec<u8>> {
        let mut encoder = Encoder::new(Vec::new(), 0).expect("Failed to create zstd encoder");
        encoder
            .set_parameter(CParameter::LiteralCompressionMode(ParamSwitch::Disable))
            .expect("Failed to set literal compression mode");
        encoder
            .set_parameter(CParameter::WindowLog(CL_WINDOW_LIMIT))
            .expect("Failed to set window log");
        encoder
            .set_parameter(CParameter::TargetCBlockSize(target_block_size))
            .expect("Failed to set target block size");
        encoder.include_checksum(false).expect("Failed to disable checksum");
        encoder.include_magicbytes(false).expect("Failed to disable magic bytes");
        encoder.include_dictid(false).expect("Failed to disable dictid");
        encoder.include_contentsize(true).expect("Failed to include content size");
        encoder
    }

    /// Computes the compression ratio for the provided bytes.
    ///
    /// This is computed as:
    /// `max(1, original_size * TX_L1_FEE_PRECISION_U256 / encoded_size)`
    pub fn compute_compression_ratio<T: AsRef<[u8]>>(bytes: &T) -> U256 {
        // Instantiate the compressor
        let mut compressor = compressor(N_BLOCK_SIZE_TARGET);

        // Set the pledged source size to the length of the bytes
        // and write the bytes to the compressor.
        let original_bytes_len = bytes.as_ref().len();
        compressor
            .set_pledged_src_size(Some(original_bytes_len as u64))
            .expect("failed to set pledged source size");
        compressor.write_all(bytes.as_ref()).expect("failed to write bytes to compressor");

        // Finish the compression and get the result.
        let result = compressor.finish().expect("failed to finish compression");
        let encoded_bytes_len = result.len();

        // Make sure that the compression ratio >= 1.0
        if encoded_bytes_len > original_bytes_len {
            return TX_L1_FEE_PRECISION_U256;
        }

        // compression_ratio(tx) = size(tx) * PRECISION / size(zstd(tx))
        U256::from(original_bytes_len)
            .saturating_mul(TX_L1_FEE_PRECISION_U256)
            .wrapping_div(U256::from(encoded_bytes_len))
    }
}

#[cfg(not(feature = "zstd_compression"))]
mod zstd_compression {
    use super::*;

    /// Computes the compression ratio for the provided bytes. This panics if the compression
    /// feature is not enabled. This is to support `no_std` environments where zstd is not
    /// available.
    pub fn compute_compression_ratio<T: AsRef<[u8]>>(_bytes: &T) -> U256 {
        panic!("Compression feature is not enabled. Please enable the 'compression' feature to use this function.");
    }
}

/// A generic wrapper for a type that includes a compression ratio and encoded bytes.
#[derive(Debug, Clone)]
pub struct WithCompressionRatio<T> {
    // The original value.
    value: T,
    // The compression ratio:
    // compression_ratio = max(1, size(v) * 1e9 / size(compress(v)))
    compression_ratio: U256,
    // The raw encoded bytes of `value`, without compression.
    encoded_bytes: Bytes,
}

/// A trait for types that can be constructed from a transaction,
/// its sender, encoded bytes and compression ratio.
pub trait FromTxWithCompressionRatio<Tx> {
    /// Builds a `TxEnv` from a transaction, its sender, encoded transaction bytes,
    /// and a compression ratio.
    fn from_tx_with_compression_ratio(
        tx: &Tx,
        sender: Address,
        encoded: Bytes,
        compression_ratio: Option<U256>,
    ) -> Self;
}

impl<TxEnv, T> FromTxWithCompressionRatio<&T> for TxEnv
where
    TxEnv: FromTxWithCompressionRatio<T>,
{
    fn from_tx_with_compression_ratio(
        tx: &&T,
        sender: Address,
        encoded: Bytes,
        compression_ratio: Option<U256>,
    ) -> Self {
        TxEnv::from_tx_with_compression_ratio(tx, sender, encoded, compression_ratio)
    }
}

impl<T, TxEnv: FromTxWithCompressionRatio<T>> IntoTxEnv<TxEnv>
    for WithCompressionRatio<Recovered<T>>
{
    fn into_tx_env(self) -> TxEnv {
        let recovered = &self.value;
        TxEnv::from_tx_with_compression_ratio(
            recovered.inner(),
            recovered.signer(),
            self.encoded_bytes.clone(),
            Some(self.compression_ratio),
        )
    }
}

impl<T, TxEnv: FromTxWithCompressionRatio<T>> IntoTxEnv<TxEnv>
    for &WithCompressionRatio<Recovered<T>>
{
    fn into_tx_env(self) -> TxEnv {
        let recovered = &self.value;
        TxEnv::from_tx_with_compression_ratio(
            recovered.inner(),
            recovered.signer(),
            self.encoded_bytes.clone(),
            Some(self.compression_ratio),
        )
    }
}

impl<T, TxEnv: FromTxWithCompressionRatio<T>> IntoTxEnv<TxEnv>
    for WithCompressionRatio<&Recovered<T>>
{
    fn into_tx_env(self) -> TxEnv {
        let recovered = &self.value;
        TxEnv::from_tx_with_compression_ratio(
            recovered.inner(),
            *recovered.signer(),
            self.encoded_bytes.clone(),
            Some(self.compression_ratio),
        )
    }
}

impl<T, TxEnv: FromTxWithCompressionRatio<T>> IntoTxEnv<TxEnv>
    for &WithCompressionRatio<&Recovered<T>>
{
    fn into_tx_env(self) -> TxEnv {
        let recovered = &self.value;
        TxEnv::from_tx_with_compression_ratio(
            recovered.inner(),
            *recovered.signer(),
            self.encoded_bytes.clone(),
            Some(self.compression_ratio),
        )
    }
}

impl FromTxWithCompressionRatio<ScrollTxEnvelope> for ScrollTransactionIntoTxEnv<TxEnv> {
    fn from_tx_with_compression_ratio(
        tx: &ScrollTxEnvelope,
        caller: Address,
        encoded: Bytes,
        compression_ratio: Option<U256>,
    ) -> Self {
        let base = match &tx {
            ScrollTxEnvelope::Legacy(tx) => TxEnv::from_recovered_tx(tx.tx(), caller),
            ScrollTxEnvelope::Eip2930(tx) => TxEnv::from_recovered_tx(tx.tx(), caller),
            ScrollTxEnvelope::Eip1559(tx) => TxEnv::from_recovered_tx(tx.tx(), caller),
            ScrollTxEnvelope::Eip7702(tx) => TxEnv::from_recovered_tx(tx.tx(), caller),
            ScrollTxEnvelope::L1Message(tx) => {
                let TxL1Message { to, value, gas_limit, input, queue_index: _, sender: _ } = &**tx;
                TxEnv {
                    tx_type: tx.ty(),
                    caller,
                    gas_limit: *gas_limit,
                    kind: TxKind::Call(*to),
                    value: *value,
                    data: input.clone(),
                    ..Default::default()
                }
            }
        };

        Self::new(base, Some(encoded), compression_ratio)
    }
}

/// A trait that allows a type to be converted into [`WithCompressionRatio`].
pub trait ToTxWithCompressionRatio<Tx> {
    /// Converts the type into a [`WithCompressionRatio`] instance using the provided compression
    /// ratio.
    fn with_compression_ratio(
        &self,
        compression_ratio: U256,
    ) -> WithCompressionRatio<Recovered<&Tx>>;
}

impl<Tx: Encodable2718> ToTxWithCompressionRatio<Tx> for Recovered<&Tx> {
    fn with_compression_ratio(
        &self,
        compression_ratio: U256,
    ) -> WithCompressionRatio<Recovered<&Tx>> {
        let encoded_bytes = self.inner().encoded_2718();
        WithCompressionRatio {
            value: *self,
            compression_ratio,
            encoded_bytes: encoded_bytes.into(),
        }
    }
}

impl<Tx, T: RecoveredTx<Tx>> RecoveredTx<Tx> for WithCompressionRatio<T> {
    fn tx(&self) -> &Tx {
        self.value.tx()
    }

    fn signer(&self) -> &Address {
        self.value.signer()
    }
}

#[cfg(test)]
mod tests {
    use super::compute_compression_ratio;
    use alloy_primitives::{bytes, U256};

    #[test]
    fn test_compute_compression_ratio() -> eyre::Result<()> {
        // eth-transfer
        // https://scrollscan.com/tx/0x8c7eba9a56e25c4402a1d9fdbe6fbe70e6f6f89484b2e4f5c329258a924193b4
        let bytes = bytes!("0x02f86b83082750830461a40183830fb782523f94802b65b5d9016621e66003aed0b16615093f328b8080c001a0a1fa6bbede5ae355eaec83fdcda65eab240476895e649576552850de726596cca0424eb1f5221865817b270d85caf8611d35ea6d7c2e86c9c31af5c06df04a2587");
        let ratio = compute_compression_ratio(&bytes);
        assert_eq!(ratio, U256::from(1_000_000_000u64)); // 1x (not compressible)

        // scr-transfer
        // https://scrollscan.com/tx/0x7b681ce914c9774aff364d2b099b2ba41dea44bcd59dbebb9d4c4b6853893179
        let bytes = bytes!("0x02f8b28308275001830f4240840279876683015c2894d29687c813d741e2f938f4ac377128810e217b1b80b844a9059cbb000000000000000000000000687b50a70d33d71f9a82dd330b8c091e4d77250800000000000000000000000000000000000000000000000ac96dda943e512bb9c080a0fdacacd07ed7c708e2193b803d731d3d288dcd39c317f321f243cd790406868ba0285444ab799632c88fd47c874c218bceb1589843949b5bc0f3ead1df069f3233");
        let ratio = compute_compression_ratio(&bytes);
        assert_eq!(ratio, U256::from(1_117_283_950u64)); // 1.1x

        // syncswap-swap
        // https://scrollscan.com/tx/0x59a7b72503400b6719f3cb670c7b1e7e45ce5076f30b98bdaad3b07a5d0fbc02
        let bytes = bytes!("0xf902cf830887d783a57282830493e09480e38291e06339d10aab483c65695d004dbd5c6980b902642cc4081e00000000000000000000000000000000000000000000000000000000000000600000000000000000000000000000000000000000000000000000000005ec79b80000000000000000000000000000000000000000000000000003328b944c400000000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000006000000000000000000000000053000000000000000000000000000000000000040000000000000000000000000000000000000000000000000091a94863ca800000000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000000000000000000000020000000000000000000000000814a23b053fd0f102aeeda0459215c2444799c7000000000000000000000000000000000000000000000000000000000000000800000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000000000000000000000000600000000000000000000000005300000000000000000000000000000000000004000000000000000000000000485ca81b70255da2fe3fd0814b57d1b08fce784e0000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000000083104ec3a050db0fbfa3fd83aa9077abdd4edb3dc504661d6fb3b39f973fe994de8fb0ac41a044983fa3d16aa0e156a1b3382fa763f9831be5a5c158f849be524d41d100ab52");
        let ratio = compute_compression_ratio(&bytes);
        assert_eq!(ratio, U256::from(3_059_322_033u64)); // 3.1x

        // uniswap-swap
        // https://scrollscan.com/tx/0x65b268bd8ef416f44983ee277d748de044243272b0f106b71ff03cc8501a05da
        let bytes = bytes!("0xf9014e830887e0836b92a7830493e094fc30937f5cde93df8d48acaf7e6f5d8d8a31f63680b8e45023b4df00000000000000000000000006efdbff2a14a7c8e15944d1f4a48f9f95f663a4000000000000000000000000530000000000000000000000000000000000000400000000000000000000000000000000000000000000000000000000000001f4000000000000000000000000485ca81b70255da2fe3fd0814b57d1b08fce784e000000000000000000000000000000000000000000000000006a94d74f43000000000000000000000000000000000000000000000000000000000000045af675000000000000000000000000000000000000000000000000000000000000000083104ec4a0a527358d5bfb89dcc7939265c6add9faf4697415174723e509f795ad44021d98a0776f4a8a8a51da98b70d960a5bd1faf3c79b8dddc0bc2c642a4c2634c6990f02");
        let ratio = compute_compression_ratio(&bytes);
        assert_eq!(ratio, U256::from(1_710_659_898u64)); // 1.7x

        // etherfi-deposit
        // https://scrollscan.com/tx/0x41a77736afd54134b6c673e967c9801e326495074012b4033bd557920cbe5a71
        let bytes = bytes!("0x02f901d58308275082f88a834c4b4084044c7166831066099462f623161fdb6564925c3f9b783cbdfef4ce8aec80b9016463baa26000000000000000000000000077a7e3215a621a9935d32a046212ebfcffa3bff900000000000000000000000006efdbff2a14a7c8e15944d1f4a48f9f95f663a400000000000000000000000008c6f91e2b681faf5e17227f2a44c307b3c1364c0000000000000000000000000000000000000000000000000000000002d4cae000000000000000000000000000000000000000000000000000000000028f7f83000000000000000000000000249e3fa81d73244f956ecd529715323b6d02f24b00000000000000000000000000000000000000000000000000000000000000e00000000000000000000000000000000000000000000000000000000000000041a95314c3a11f86cc673f2afd60d27f559cb2edcc0da5af030adffc97f9a5edc3314efbadd32878e289017f644a4afa365da5367fefe583f7c4ff0c6047e2c1ff1b00000000000000000000000000000000000000000000000000000000000000c080a05b2d22b8aaf6d334471e74899cfc4c81186f8a94f278c97ee211727d8027ceafa0450352d9a1782180c27a03889d317d31e725dda38a5bd3c0531950b879ed50a1");
        let ratio = compute_compression_ratio(&bytes);
        assert_eq!(ratio, U256::from(1_496_835_443u64)); // 1.4x

        // edgepushoracle-postupdate
        // https://scrollscan.com/tx/0x8271c68146a3b07b1ebf52ce0b550751f49cbd72fa0596ef14ff56d1f23a0bec
        let bytes = bytes!("0xf9046f83015e4c836e0b6b8303cbf8946a5b3ab3274b738eab25205af6e2d4dd7781292480b9040449a1a4fb000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000c000000000000000000000000000000000000000000000000000000000000000600000000000000000000000000000000000000000000000000000000005f725f60000000000000000000000000000000000000000000000000000000003d0cac600000000000000000000000000000000000000000000000000000000685d50cd000000000000000000000000000000000000000000000000000000000000000500000000000000000000000000000000000000000000000000000000000000a0000000000000000000000000000000000000000000000000000000000000012000000000000000000000000000000000000000000000000000000000000001a0000000000000000000000000000000000000000000000000000000000000022000000000000000000000000000000000000000000000000000000000000002a0000000000000000000000000000000000000000000000000000000000000004155903b95865fc5a5dd7d4d876456140dd0b815695647fc41eb1924f4cfe267265130b5a5d77125c44cf6a5a81edba6d5850ba00f90ab83281c9b44e17528fd74010000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000416f000e0498733998e6a1a6454e116c1b1f95f7e000400b6a54029406cf288bdc615b62de8e2db533d6010ca57001e0b8a4b3f05ed516a31830516c52b9df206e000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000410dabc77a807d729ff62c3be740d492d884f026ad2770fa7c4bdec569e201643656b07f2009d2129173738571417734a3df051cebc7b8233bec6d9471c21c098700000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000041eb009614c939170e9ff3d3e06c3a2c45810fe46a364ce28ecec5e220f5fd86cd6e0f70ab9093dd6b22b69980246496b600c8fcb054047962d4128efa48b692f301000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000041a31b4dd4f0a482372d75c7a8c5f11aa8084a5f358579866f1d25a26a15beb2b5153400bfa7fa3d6fba138c02dd1eb8a5a97d62178d98c5632a153396a566e5ed000000000000000000000000000000000000000000000000000000000000000083104ec4a05cb4eee77676d432c672008594825b957d34ae5dd786ed294501849bb1ce285aa01325f37cdc945863ec0474932102bc944cb98a663db6d30ae23c2ebb5f9ce070");
        let ratio = compute_compression_ratio(&bytes);
        assert_eq!(ratio, U256::from(2_139_097_744u64)); // 2.1x

        Ok(())
    }
}
