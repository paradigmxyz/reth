// SPDX-License-Identifier: MIT
// Original source: https://github.com/JonahGroendal/asn1-decode
pragma solidity 0.8.24;

// Inspired by PufferFinance/rave - Apache-2.0 license
// https://github.com/JonahGroendal/asn1-decode/blob/5c2d1469fc678513753786acb441e597969192ec/contracts/Asn1Decode.sol

import "./BytesUtils.sol";

/// @title NodePtr
/// @custom:security-contact security@taiko.xyz
library NodePtr {
    // Unpack first byte index
    function ixs(uint256 self) internal pure returns (uint256) {
        return uint80(self);
    }

    // Unpack first content byte index
    function ixf(uint256 self) internal pure returns (uint256) {
        return uint80(self >> 80);
    }

    // Unpack last content byte index
    function ixl(uint256 self) internal pure returns (uint256) {
        return uint80(self >> 160);
    }

    // Pack 3 uint80s into a uint256
    function getPtr(uint256 _ixs, uint256 _ixf, uint256 _ixl) internal pure returns (uint256) {
        _ixs |= _ixf << 80;
        _ixs |= _ixl << 160;
        return _ixs;
    }
}

/// @title Asn1Decode
/// @custom:security-contact security@taiko.xyz
library Asn1Decode {
    using NodePtr for uint256;
    using BytesUtils for bytes;

    /*
    * @dev Get the root node. First step in traversing an ASN1 structure
    * @param der The DER-encoded ASN1 structure
    * @return A pointer to the outermost node
    */
    function root(bytes memory der) internal pure returns (uint256) {
        return _readNodeLength(der, 0);
    }

    /*
    * @dev Get the root node of an ASN1 structure that's within an octet string value
    * @param der The DER-encoded ASN1 structure
    * @return A pointer to the outermost node
    */
    function rootOfOctetStringAt(bytes memory der, uint256 ptr) internal pure returns (uint256) {
        require(der[ptr.ixs()] == 0x04, "Not type OCTET STRING");
        return _readNodeLength(der, ptr.ixf());
    }

    /*
    * @dev Get the next sibling node
    * @param der The DER-encoded ASN1 structure
    * @param ptr Points to the indices of the current node
    * @return A pointer to the next sibling node
    */
    function nextSiblingOf(bytes memory der, uint256 ptr) internal pure returns (uint256) {
        return _readNodeLength(der, ptr.ixl() + 1);
    }

    /*
    * @dev Get the first child node of the current node
    * @param der The DER-encoded ASN1 structure
    * @param ptr Points to the indices of the current node
    * @return A pointer to the first child node
    */
    function firstChildOf(bytes memory der, uint256 ptr) internal pure returns (uint256) {
        require(der[ptr.ixs()] & 0x20 == 0x20, "Not a constructed type");
        return _readNodeLength(der, ptr.ixf());
    }

    /*
    * @dev Extract value of node from DER-encoded structure
    * @param der The der-encoded ASN1 structure
    * @param ptr Points to the indices of the current node
    * @return Value bytes of node
    */
    function bytesAt(bytes memory der, uint256 ptr) internal pure returns (bytes memory) {
        return der.substring(ptr.ixf(), ptr.ixl() + 1 - ptr.ixf());
    }

    /*
    * @dev Extract entire node from DER-encoded structure
    * @param der The DER-encoded ASN1 structure
    * @param ptr Points to the indices of the current node
    * @return All bytes of node
    */
    function allBytesAt(bytes memory der, uint256 ptr) internal pure returns (bytes memory) {
        return der.substring(ptr.ixs(), ptr.ixl() + 1 - ptr.ixs());
    }

    function keccakOfBytesAt(bytes memory der, uint256 ptr) internal pure returns (bytes32) {
        return der.keccak(ptr.ixf(), ptr.ixl() + 1 - ptr.ixf());
    }

    function keccakOfAllBytesAt(bytes memory der, uint256 ptr) internal pure returns (bytes32) {
        return der.keccak(ptr.ixs(), ptr.ixl() + 1 - ptr.ixs());
    }

    function _readNodeLength(bytes memory der, uint256 ix) private pure returns (uint256) {
        uint256 length;
        uint80 ixFirstContentByte;
        uint80 ixLastContentByte;
        if ((der[ix + 1] & 0x80) == 0) {
            length = uint8(der[ix + 1]);
            ixFirstContentByte = uint80(ix + 2);
            ixLastContentByte = uint80(ixFirstContentByte + length - 1);
        } else {
            uint8 lengthbytesLength = uint8(der[ix + 1] & 0x7F);
            if (lengthbytesLength == 1) {
                length = der.readUint8(ix + 2);
            } else if (lengthbytesLength == 2) {
                length = der.readUint16(ix + 2);
            } else {
                length = uint256(
                    der.readBytesN(ix + 2, lengthbytesLength) >> (32 - lengthbytesLength) * 8
                );
            }
            ixFirstContentByte = uint80(ix + 2 + lengthbytesLength);
            ixLastContentByte = uint80(ixFirstContentByte + length - 1);
        }
        return NodePtr.getPtr(ix, ixFirstContentByte, ixLastContentByte);
    }
}
