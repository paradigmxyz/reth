// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import "@openzeppelin/contracts/utils/Strings.sol";

/// @title LibBridgedToken
/// @custom:security-contact security@taiko.xyz
library LibBridgedToken {
    error BTOKEN_INVALID_PARAMS();
    error BTOKEN_INVALID_TO_ADDR();

    function validateInputs(address _srcToken, uint256 _srcChainId) internal view {
        if (_srcToken == address(0) || _srcChainId == 0 || _srcChainId == block.chainid) {
            revert BTOKEN_INVALID_PARAMS();
        }
    }

    function checkToAddress(address _to) internal view {
        if (_to == address(this)) revert BTOKEN_INVALID_TO_ADDR();
    }

    function buildURI(
        address _srcToken,
        uint256 _srcChainId,
        string memory _extraParams
    )
        internal
        pure
        returns (string memory)
    {
        // Creates a base URI in the format specified by EIP-681:
        // https://eips.ethereum.org/EIPS/eip-681
        return string(
            abi.encodePacked(
                "ethereum:",
                Strings.toHexString(uint160(_srcToken), 20),
                "@",
                Strings.toString(_srcChainId),
                "/tokenURI?uint256=",
                _extraParams
            )
        );
    }
}
