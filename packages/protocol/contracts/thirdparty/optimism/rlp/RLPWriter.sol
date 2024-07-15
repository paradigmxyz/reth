// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

/// @custom:attribution https://github.com/bakaoh/solidity-rlp-encode
/// @title RLPWriter
/// @author RLPWriter is a library for encoding Solidity types to RLP bytes. Adapted from Bakaoh's
///         RLPEncode library (https://github.com/bakaoh/solidity-rlp-encode) with minor
///         modifications to improve legibility. (A shout-out to Optimism !)
library RLPWriter {
    /// @notice RLP encodes a byte string.
    /// @param _in The byte string to encode.
    /// @return out_ The RLP encoded string in bytes.
    function writeBytes(bytes memory _in) internal pure returns (bytes memory out_) {
        if (_in.length == 1 && uint8(_in[0]) < 128) {
            out_ = _in;
        } else {
            out_ = abi.encodePacked(_writeLength(_in.length, 128), _in);
        }
    }

    /// @notice RLP encodes a uint.
    /// @param _in The uint256 to encode.
    /// @return out_ The RLP encoded uint256 in bytes.
    function writeUint(uint256 _in) internal pure returns (bytes memory out_) {
        out_ = writeBytes(_toBinary(_in));
    }

    /// @notice Encode the first byte and then the `len` in binary form if `length` is more than 55.
    /// @param _len    The length of the string or the payload.
    /// @param _offset 128 if item is string, 192 if item is list.
    /// @return out_ RLP encoded bytes.
    function _writeLength(uint256 _len, uint256 _offset) private pure returns (bytes memory out_) {
        if (_len < 56) {
            out_ = new bytes(1);
            out_[0] = bytes1(uint8(_len) + uint8(_offset));
        } else {
            uint256 lenLen;
            uint256 i = 1;
            while (_len / i != 0) {
                lenLen++;
                i *= 256;
            }

            out_ = new bytes(lenLen + 1);
            out_[0] = bytes1(uint8(lenLen) + uint8(_offset) + 55);
            for (i = 1; i <= lenLen; i++) {
                out_[i] = bytes1(uint8((_len / (256 ** (lenLen - i))) % 256));
            }
        }
    }

    /// @notice Encode integer in big endian binary form with no leading zeroes.
    /// @param _x The integer to encode.
    /// @return out_ RLP encoded bytes.
    function _toBinary(uint256 _x) private pure returns (bytes memory out_) {
        bytes memory b = abi.encodePacked(_x);

        uint256 i = 0;
        for (; i < 32; i++) {
            if (b[i] != 0) {
                break;
            }
        }

        out_ = new bytes(32 - i);
        for (uint256 j = 0; j < out_.length; j++) {
            out_[j] = b[i++];
        }
    }
}
