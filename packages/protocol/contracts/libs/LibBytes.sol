// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

library LibBytes {
    error INNER_ERROR(bytes innerError);

    // Function body taken from:
    // https://github.com/clober-dex/core/blob/main/contracts/utils/BoringERC20.sol#L17-L33
    /// @notice Function to convert returned data to string
    /// returns '' as fallback value.
    function toString(bytes memory _data) internal pure returns (string memory) {
        if (_data.length >= 64) {
            return abi.decode(_data, (string));
        } else if (_data.length == 32) {
            uint8 i = 0;
            while (i < 32 && _data[i] != 0) {
                i++;
            }
            bytes memory bytesArray = new bytes(i);
            for (i = 0; i < 32 && _data[i] != 0; i++) {
                bytesArray[i] = _data[i];
            }
            return string(bytesArray);
        } else {
            return "";
        }
    }

    // Taken from:
    // https://github.com/boringcrypto/BoringSolidity/blob/master/contracts/BoringBatchable.sol
    /// @dev Helper function to extract a useful revert message from a failed call.
    /// If the returned data is malformed or not correctly abi encoded then this call can fail
    /// itself.
    function revertWithExtractedError(bytes memory _returnData) internal pure {
        // If the _res length is less than 68, then
        // the transaction failed with custom error or silently (without a revert message)
        if (_returnData.length < 68) revert INNER_ERROR(_returnData);

        assembly {
            // Slice the sighash.
            _returnData := add(_returnData, 0x04)
        }
        revert(abi.decode(_returnData, (string))); // All that remains is the revert string
    }
}
