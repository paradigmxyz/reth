// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

import "../../common/EssentialContract.sol";
import "../TaikoData.sol";

/// @title Guardians
abstract contract Guardians is EssentialContract {
    uint256 public constant MIN_NUM_GUARDIANS = 5;

    mapping(address guardian => uint256 id) public guardianIds; // slot 1
    mapping(uint32 version => mapping(bytes32 => uint256 approvalBits)) internal _approvals;
    address[] public guardians; // slot 3
    uint32 public version; // slot 4
    uint32 public minGuardians;

    uint256[46] private __gap;

    event GuardiansUpdated(uint32 version, address[] guardians);
    event Approved(uint256 indexed operationId, uint256 approvalBits, bool proofSubmitted);

    error INVALID_GUARDIAN();
    error INVALID_GUARDIAN_SET();
    error INVALID_MIN_GUARDIANS();
    error INVALID_PROOF();

    /// @notice Set the set of guardians
    /// @param _guardians The new set of guardians
    function setGuardians(
        address[] memory _guardians,
        uint8 _minGuardians
    )
        external
        onlyOwner
        nonReentrant
    {
        if (_guardians.length < MIN_NUM_GUARDIANS || _guardians.length > type(uint8).max) {
            revert INVALID_GUARDIAN_SET();
        }
        if (
            _minGuardians == 0 || _minGuardians < _guardians.length / 2
                || _minGuardians > _guardians.length
        ) revert INVALID_MIN_GUARDIANS();

        // Delete current guardians data
        for (uint256 i; i < guardians.length; ++i) {
            delete guardianIds[guardians[i]];
        }
        assembly {
            sstore(guardians.slot, 0)
        }

        for (uint256 i = 0; i < _guardians.length;) {
            address guardian = _guardians[i];
            if (guardian == address(0)) revert INVALID_GUARDIAN();
            if (guardianIds[guardian] != 0) revert INVALID_GUARDIAN_SET();

            // Save and index the guardian
            guardians.push(guardian);
            guardianIds[guardian] = ++i;
        }

        minGuardians = _minGuardians;
        emit GuardiansUpdated(++version, _guardians);
    }

    function isApproved(bytes32 hash) public view returns (bool) {
        return isApproved(_approvals[version][hash]);
    }

    function numGuardians() public view returns (uint256) {
        return guardians.length;
    }

    function approve(uint256 operationId, bytes32 hash) internal returns (bool approved) {
        uint256 id = guardianIds[msg.sender];
        if (id == 0) revert INVALID_GUARDIAN();

        unchecked {
            _approvals[version][hash] |= 1 << (id - 1);
        }

        approved = isApproved(_approvals[version][hash]);
        emit Approved(operationId, _approvals[version][hash], approved);
    }

    function deleteApproval(bytes32 hash) internal {
        delete _approvals[version][hash];
    }

    function isApproved(uint256 approvalBits) internal view returns (bool) {
        uint256 count;
        uint256 bits = approvalBits;
        unchecked {
            for (uint256 i; i < guardians.length; ++i) {
                if (bits & 1 == 1) ++count;
                if (count == minGuardians) return true;
                bits >>= 1;
            }
        }
        return false;
    }
}
