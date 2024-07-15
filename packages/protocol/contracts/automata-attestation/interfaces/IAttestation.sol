//SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import { V3Struct } from "../lib/QuoteV3Auth/V3Struct.sol";

/// @title IAttestation
/// @custom:security-contact security@taiko.xyz
interface IAttestation {
    function verifyAttestation(bytes calldata data) external returns (bool);
    function verifyParsedQuote(V3Struct.ParsedV3QuoteStruct calldata v3quote)
        external
        returns (bool success, bytes memory retData);
}
