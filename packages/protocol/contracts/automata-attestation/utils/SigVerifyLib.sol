// SPDX-License-Identifier: GPL-3.0
pragma solidity 0.8.24;

import "../interfaces/ISigVerifyLib.sol";
import "./BytesUtils.sol";

/// @title SigVerifyLib
/// @custom:security-contact security@taiko.xyz
// Library for verifying signatures
contract SigVerifyLib is ISigVerifyLib {
    using BytesUtils for bytes;

    address private immutable __es256Verifier;

    constructor(address es256Verifier) {
        __es256Verifier = es256Verifier;
    }

    function verifyES256Signature(
        bytes calldata tbs,
        bytes calldata signature,
        bytes calldata publicKey
    )
        external
        view
        returns (bool sigValid)
    {
        // Parse signature
        if (signature.length != 64) {
            return false;
        }
        uint256 r = uint256(bytes32(signature.substring(0, 32)));
        uint256 s = uint256(bytes32(signature.substring(32, 32)));
        // Parse public key
        if (publicKey.length != 64) {
            return false;
        }
        uint256 gx = uint256(bytes32(publicKey.substring(0, 32)));
        uint256 gy = uint256(bytes32(publicKey.substring(32, 32)));

        // Verify signature
        bytes memory args = abi.encode(sha256(tbs), r, s, gx, gy);
        (bool success, bytes memory ret) = __es256Verifier.staticcall(args);
        assert(success); // never reverts, always returns 0 or 1

        return abi.decode(ret, (uint256)) == 1;
    }
}
