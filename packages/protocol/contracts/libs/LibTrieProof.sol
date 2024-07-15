// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity 0.8.24;

import "../thirdparty/optimism/rlp/RLPReader.sol";
import "../thirdparty/optimism/rlp/RLPWriter.sol";
import "../thirdparty/optimism/trie/SecureMerkleTrie.sol";

/// @title LibTrieProof
/// @custom:security-contact security@taiko.xyz
library LibTrieProof {
    // The consensus format representing account is RLP encoded in the
    // following order: nonce, balance, storageHash, codeHash.
    uint256 private constant _ACCOUNT_FIELD_INDEX_STORAGE_HASH = 2;

    error LTP_INVALID_ACCOUNT_PROOF();
    error LTP_INVALID_INCLUSION_PROOF();

    /// @notice Verifies that the value of a slot in the storage of an account is value.
    ///
    /// @param _rootHash The merkle root of state tree or the account tree. If accountProof's length
    /// is zero, it is used as the account's storage root, otherwise it will be used as the state
    /// root.
    /// @param _addr The address of contract.
    /// @param _slot The slot in the contract.
    /// @param _value The value to be verified.
    /// @param _accountProof The account proof
    /// @param _storageProof The storage proof
    /// @return storageRoot_ The account's storage root
    function verifyMerkleProof(
        bytes32 _rootHash,
        address _addr,
        bytes32 _slot,
        bytes32 _value,
        bytes[] memory _accountProof,
        bytes[] memory _storageProof
    )
        internal
        pure
        returns (bytes32 storageRoot_)
    {
        if (_accountProof.length != 0) {
            bytes memory rlpAccount =
                SecureMerkleTrie.get(abi.encodePacked(_addr), _accountProof, _rootHash);

            if (rlpAccount.length == 0) revert LTP_INVALID_ACCOUNT_PROOF();

            RLPReader.RLPItem[] memory accountState = RLPReader.readList(rlpAccount);

            storageRoot_ =
                bytes32(RLPReader.readBytes(accountState[_ACCOUNT_FIELD_INDEX_STORAGE_HASH]));
        } else {
            storageRoot_ = _rootHash;
        }

        bool verified = SecureMerkleTrie.verifyInclusionProof(
            bytes.concat(_slot), RLPWriter.writeUint(uint256(_value)), _storageProof, storageRoot_
        );

        if (!verified) revert LTP_INVALID_INCLUSION_PROOF();
    }
}
