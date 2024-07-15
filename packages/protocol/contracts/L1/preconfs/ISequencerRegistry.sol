// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

/// @title ISequencerRegistry
/// @custom:security-contact security@taiko.xyz
interface ISequencerRegistry {
    /// @notice Return true if the specified address can propose blocks, false otherwise
    /// @param _proposer The address proposing a block
    function isEligibleSigner(address _proposer) external returns (bool);
}
