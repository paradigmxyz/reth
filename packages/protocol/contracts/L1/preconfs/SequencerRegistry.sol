// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import "../../common/EssentialContract.sol";
import "./ISequencerRegistry.sol";

/// @title SequencerRegistry
/// A dummy implementation that only whitelist some trusted addresses. A real
/// implementation would only allow a single proposer address to propose a block
/// using some selection mechanism.
/// @custom:security-contact security@taiko.xyz
contract SequencerRegistry is EssentialContract, ISequencerRegistry {
    /// @dev Emitted when the status of a sequencer is updated.
    /// @param sequencer The address of the sequencer whose state has updated.
    /// @param enabled If the sequencer is now enabled or not.
    event SequencerUpdated(address indexed sequencer, bool enabled);

    /// @notice Whitelisted sequencers
    mapping(address sequencer => bool enabled) public sequencers;

    uint256[49] private __gap;

    /// @notice Initializes the contract with the provided address manager.
    /// @param _owner The address of the owner.
    function init(address _owner) external initializer {
        __Essential_init(_owner);
    }

    /// @notice Sets/unsets an the imageId as trusted entity
    /// @param _sequencers The list of sequencers
    /// @param _enabled The corresponding list of the new status of the sequencers
    function setSequencers(
        address[] memory _sequencers,
        bool[] memory _enabled
    )
        external
        onlyOwner
    {
        require(_sequencers.length == _enabled.length, "invalid input data");
        for (uint256 i = 0; i < _sequencers.length; i++) {
            sequencers[_sequencers[i]] = _enabled[i];
            emit SequencerUpdated(_sequencers[i], _enabled[i]);
        }
    }

    /// @inheritdoc ISequencerRegistry
    function isEligibleSigner(address _proposer) external view returns (bool) {
        return sequencers[_proposer];
    }
}
