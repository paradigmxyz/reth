// SPDX-License-Identifier: BSD 2-Clause License
pragma solidity 0.8.24;

// Inspired by ensdomains/dnssec-oracle - BSD-2-Clause license
// https://github.com/ensdomains/dnssec-oracle/blob/master/contracts/BytesUtils.sol
/// @title BytesUtils
/// @custom:security-contact security@taiko.xyz
library BytesUtils {
    /*
    * @dev Returns the keccak-256 hash of a byte range.
    * @param self The byte string to hash.
    * @param offset The position to start hashing at.
    * @param len The number of bytes to hash.
    * @return The hash of the byte range.
    */
    function keccak(
        bytes memory self,
        uint256 offset,
        uint256 len
    )
        internal
        pure
        returns (bytes32 ret)
    {
        require(offset + len <= self.length, "invalid offset");
        assembly {
            ret := keccak256(add(add(self, 32), offset), len)
        }
    }

    /*
    * @dev Returns true if the two byte ranges are equal.
    * @param self The first byte range to compare.
    * @param offset The offset into the first byte range.
    * @param other The second byte range to compare.
    * @param otherOffset The offset into the second byte range.
    * @param len The number of bytes to compare
    * @return true if the byte ranges are equal, false otherwise.
    */
    function equals(
        bytes memory self,
        uint256 offset,
        bytes memory other,
        uint256 otherOffset,
        uint256 len
    )
        internal
        pure
        returns (bool)
    {
        return keccak(self, offset, len) == keccak(other, otherOffset, len);
    }

    /*
    * @dev Returns the 8-bit number at the specified index of self.
    * @param self The byte string.
    * @param idx The index into the bytes
    * @return The specified 8 bits of the string, interpreted as an integer.
    */
    function readUint8(bytes memory self, uint256 idx) internal pure returns (uint8 ret) {
        return uint8(self[idx]);
    }

    /*
    * @dev Returns the 16-bit number at the specified index of self.
    * @param self The byte string.
    * @param idx The index into the bytes
    * @return The specified 16 bits of the string, interpreted as an integer.
    */
    function readUint16(bytes memory self, uint256 idx) internal pure returns (uint16 ret) {
        require(idx + 2 <= self.length, "invalid idx");
        assembly {
            ret := and(mload(add(add(self, 2), idx)), 0xFFFF)
        }
    }

    /*
    * @dev Returns the n byte value at the specified index of self.
    * @param self The byte string.
    * @param idx The index into the bytes.
    * @param len The number of bytes.
    * @return The specified 32 bytes of the string.
    */
    function readBytesN(
        bytes memory self,
        uint256 idx,
        uint256 len
    )
        internal
        pure
        returns (bytes32 ret)
    {
        require(len <= 32, "unexpected len");
        require(idx + len <= self.length, "unexpected idx");
        assembly {
            let mask := not(sub(exp(256, sub(32, len)), 1))
            ret := and(mload(add(add(self, 32), idx)), mask)
        }
    }

    function memcpy(uint256 dest, uint256 src, uint256 len) private pure {
        assembly {
            mcopy(dest, src, len)
        }
    }

    /*
    * @dev Copies a substring into a new byte string.
    * @param self The byte string to copy from.
    * @param offset The offset to start copying at.
    * @param len The number of bytes to copy.
    */
    function substring(
        bytes memory self,
        uint256 offset,
        uint256 len
    )
        internal
        pure
        returns (bytes memory)
    {
        require(offset + len <= self.length, "unexpected offset");

        bytes memory ret = new bytes(len);
        uint256 dest;
        uint256 src;

        assembly {
            dest := add(ret, 32)
            src := add(add(self, 32), offset)
        }
        memcpy(dest, src, len);

        return ret;
    }

    function compareBytes(bytes memory a, bytes memory b) internal pure returns (bool) {
        return keccak256(a) == keccak256(b);
    }
}
