use crate::Compact;
use alloy_eips::eip4895::Withdrawal;
use alloy_primitives::Address;
use bytes::Buf;
use modular_bitfield::{
    error,
    prelude::{Specifier, B4},
    private::write_specifier,
};

/// Implement `Compact` for `Withdrawal`.
impl Compact for Withdrawal {
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let mut flags = WithdrawalFlags::default();
        let mut buffer = bytes::BytesMut::new();
        let index_len = self.index.to_compact(&mut buffer);
        flags.set_index_len(index_len as u8);
        let validator_index_len = self.validator_index.to_compact(&mut buffer);
        flags.set_validator_index_len(validator_index_len as u8);
        let _address_len = self.address.to_compact(&mut buffer);
        let amount_len = self.amount.to_compact(&mut buffer);
        flags.set_amount_len(amount_len as u8);
        let flags = flags.into_bytes();
        let total_length = flags.len() + buffer.len();
        buf.put_slice(&flags);
        buf.put(buffer);
        total_length
    }

    fn from_compact(buf: &[u8], _: usize) -> (Self, &[u8]) {
        let (flags, mut buf) = WithdrawalFlags::from(buf);
        let (index, new_buf) = u64::from_compact(buf, flags.index_len() as usize);
        buf = new_buf;
        let (validator_index, new_buf) =
            u64::from_compact(buf, flags.validator_index_len() as usize);
        buf = new_buf;
        let (address, new_buf) = Address::from_compact(buf, buf.len());
        buf = new_buf;
        let (amount, new_buf) = u64::from_compact(buf, flags.amount_len() as usize);
        buf = new_buf;
        let obj = Withdrawal { index, validator_index, address, amount };
        (obj, buf)
    }
}

/// Flags to store metadata about compacted Withdrawal.
pub(crate) struct WithdrawalFlags {
    bytes: [u8; 2], // Calculated from B4 Specifier
}

impl Default for WithdrawalFlags {
    #[inline]
    fn default() -> WithdrawalFlags {
        WithdrawalFlags { bytes: Default::default() }
    }
}

impl WithdrawalFlags {
    #[inline]
    pub(crate) const fn into_bytes(self) -> [u8; 2] {
        self.bytes
    }

    #[inline]
    pub(crate) const fn from_bytes(bytes: [u8; 2]) -> Self {
        Self { bytes }
    }
}

impl WithdrawalFlags {
    /// Returns the value of index_len.
    #[inline]
    pub(crate) fn index_len(&self) -> <B4 as Specifier>::InOut {
        let __bf_read = modular_bitfield::private::read_specifier::<B4>(&self.bytes[..], 0usize);
        <B4 as Specifier>::from_bytes(__bf_read)
            .expect("value contains invalid bit pattern for field WithdrawalFlags.index_len")
    }

    /// Sets the value of index_len to the given value.
    #[inline]
    pub(crate) fn set_index_len(&mut self, new_val: <B4 as Specifier>::InOut) {
        self.set_field(0, new_val)
            .expect("value out of bounds for field WithdrawalFlags.amount_len")
    }

    /// Returns the value of validator_index_len.
    #[inline]
    pub(crate) fn validator_index_len(&self) -> <B4 as Specifier>::InOut {
        let __bf_read: <B4 as Specifier>::Bytes = {
            modular_bitfield::private::read_specifier::<B4>(
                &self.bytes[..],
                <B4 as Specifier>::BITS,
            )
        };
        <B4 as Specifier>::from_bytes(__bf_read).expect(
            "value contains invalid bit pattern for field WithdrawalFlags.validator_index_len",
        )
    }

    /// Sets the value of validator_index_len to the given value.
    #[inline]
    pub(crate) fn set_validator_index_len(&mut self, new_val: <B4 as Specifier>::InOut) {
        self.set_field(1, new_val)
            .expect("value out of bounds for field WithdrawalFlags.amount_len")
    }

    /// Returns the value of amount_len.
    #[inline]
    pub(crate) fn amount_len(&self) -> <B4 as Specifier>::InOut {
        let __bf_read: <B4 as Specifier>::Bytes = {
            modular_bitfield::private::read_specifier::<B4>(
                &self.bytes[..],
                <B4 as Specifier>::BITS + <B4 as Specifier>::BITS,
            )
        };
        <B4 as Specifier>::from_bytes(__bf_read)
            .expect("value contains invalid bit pattern for field WithdrawalFlags.amount_len")
    }

    /// Sets the value of amount_len to the given value.
    #[inline]
    pub(crate) fn set_amount_len(&mut self, new_val: <B4 as Specifier>::InOut) {
        self.set_field(2, new_val)
            .expect("value out of bounds for field WithdrawalFlags.amount_len")
    }

    #[inline]
    fn set_field(
        &mut self,
        spec_bits_multiplier: usize,
        new_val: <B4 as Specifier>::InOut,
    ) -> Result<(), error::OutOfBounds> {
        let base_bits: usize = 8 * core::mem::size_of::<<B4 as Specifier>::Bytes>();
        let max_value: <B4 as Specifier>::Bytes = !0 >> (base_bits - <B4 as Specifier>::BITS);
        let spec_bits: usize = <B4 as Specifier>::BITS;
        let raw_val: <B4 as Specifier>::Bytes = <B4 as Specifier>::into_bytes(new_val)?;

        if !(base_bits == spec_bits || raw_val <= max_value) {
            return Err(error::OutOfBounds);
        }

        write_specifier::<B4>(&mut self.bytes[..], spec_bits * spec_bits_multiplier, raw_val);
        Ok(())
    }
}

impl WithdrawalFlags {
    /// Deserializes this fieldset and returns it, alongside the original slice in an advanced
    /// position.
    pub(crate) fn from(mut buf: &[u8]) -> (Self, &[u8]) {
        (WithdrawalFlags::from_bytes([buf.get_u8(), buf.get_u8()]), buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::proptest;

    proptest! {
        #[test]
        fn roundtrip(withdrawal: Withdrawal) {
            let mut compacted_withdrawal = Vec::<u8>::new();
            let len = withdrawal.to_compact(&mut compacted_withdrawal);
            let (decoded, _) = Withdrawal::from_compact(&compacted_withdrawal, len);
            assert_eq!(withdrawal, decoded)
        }
    }
}
