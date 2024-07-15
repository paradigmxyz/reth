// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import "../../common/EssentialContract.sol";
import "../../common/LibStrings.sol";
import "../verifiers/IVerifier.sol";
import "../ITaikoL1.sol";

/// @dev IMPORTANT NOTICE!!
/// @title GuardianProver - brought over from taiko-mono repository BUT MADE SOME CHANGES to make it
/// compiling. If we won't use, we can just delete/ignore this.
/// This prover uses itself as the verifier.
/// @custom:security-contact security@taiko.xyz
contract GuardianProver is EssentialContract {
    /// @notice Contains the index of the guardian in `guardians` plus one (zero means not a
    /// guardian)
    /// @dev Slot 1
    mapping(address guardian => uint256 id) public guardianIds;

    /// @notice Mapping to store the approvals for a given hash, for a given version
    mapping(uint256 version => mapping(bytes32 proofHash => uint256 approvalBits)) public approvals;

    /// @notice The set of guardians
    /// @dev Slot 3
    address[] public guardians;

    /// @notice The version of the guardians
    /// @dev Slot 4
    uint32 public version;

    /// @notice The minimum number of guardians required to approve
    uint32 public minGuardians;

    /// @notice True to enable pausing taiko proving upon conflicting proofs
    bool public provingAutoPauseEnabled;

    /// @notice Mapping from blockId to its latest proof hash
    /// @dev Slot 5
    mapping(uint256 version => mapping(uint256 blockId => bytes32 hash)) public latestProofHash;

    uint256[45] private __gap;

    /// @notice Emitted when a guardian proof is approved.
    /// @param addr The address of the guardian.
    /// @param blockId The block ID.
    /// @param blockHash The block hash.
    /// @param approved If the proof is approved.
    /// @param proofData The proof data.
    event GuardianApproval(
        address indexed addr,
        uint256 indexed blockId,
        bytes32 indexed blockHash,
        bool approved,
        bytes proofData
    );

    /// @notice Emitted when the set of guardians is updated
    /// @param version The new version
    /// @param guardians The new set of guardians
    event GuardiansUpdated(uint32 version, address[] guardians);

    /// @notice Emitted when an approval is made
    /// @param operationId The operation ID
    /// @param approvalBits The new approval bits
    /// @param minGuardiansReached If the proof was submitted
    event Approved(uint256 indexed operationId, uint256 approvalBits, bool minGuardiansReached);

    /// @notice Emitted when a guardian prover submit a different proof for the same block
    /// @param blockId The block ID
    /// @param guardian The guardian prover address
    /// @param currentProofHash The existing proof hash
    /// @param newProofHash The new and different proof hash
    /// @param provingPaused True if TaikoL1's proving is paused.
    event ConflictingProofs(
        uint256 indexed blockId,
        address indexed guardian,
        bytes32 currentProofHash,
        bytes32 newProofHash,
        bool provingPaused
    );

    /// @notice Emitted when auto pausing is enabled.
    /// @param enabled True if TaikoL1 proving auto-pause is enabled.
    event ProvingAutoPauseEnabled(bool indexed enabled);

    error GP_INVALID_GUARDIAN();
    error GP_INVALID_GUARDIAN_SET();
    error GP_INVALID_MIN_GUARDIANS();
    error GP_INVALID_STATUS();
    error GV_PERMISSION_DENIED();
    error GV_ZERO_ADDRESS();

    /// @notice Initializes the contract.
    /// @param _owner The owner of this contract. msg.sender will be used if this value is zero.
    /// @param _addressManager The address of the {AddressManager} contract.
    function init(address _owner, address _addressManager) external initializer {
        __Essential_init(_owner, _addressManager);
    }

    /// @notice Set the set of guardians
    /// @param _newGuardians The new set of guardians
    /// @param _minGuardians The minimum required to sign
    /// @param _clearData true to invalidate all existing data.
    function setGuardians(
        address[] memory _newGuardians,
        uint8 _minGuardians,
        bool _clearData
    )
        external
        onlyOwner
    {
        // We need at most 255 guardians (so the approval bits fit in a uint256)
        if (_newGuardians.length == 0 || _newGuardians.length > type(uint8).max) {
            revert GP_INVALID_GUARDIAN_SET();
        }
        // Minimum number of guardians to approve is at least equal or greater than half the
        // guardians (rounded up) and less or equal than the total number of guardians
        if (_minGuardians == 0 || _minGuardians > _newGuardians.length) {
            revert GP_INVALID_MIN_GUARDIANS();
        }

        // Delete the current guardians
        for (uint256 i; i < guardians.length; ++i) {
            delete guardianIds[guardians[i]];
        }
        delete guardians;

        // Set the new guardians
        for (uint256 i; i < _newGuardians.length; ++i) {
            address guardian = _newGuardians[i];
            if (guardian == address(0)) revert GP_INVALID_GUARDIAN();
            // This makes sure there are not duplicate addresses
            if (guardianIds[guardian] != 0) revert GP_INVALID_GUARDIAN_SET();

            // Save and index the guardian
            guardians.push(guardian);
            guardianIds[guardian] = guardians.length;
        }

        // Bump the version so previous approvals get invalidated
        if (_clearData) ++version;

        minGuardians = _minGuardians;
        emit GuardiansUpdated(version, _newGuardians);
    }

    /// @dev Enables or disables proving auto pause.
    /// @param _enable true to enable, false to disable.
    function enableProvingAutoPause(bool _enable) external onlyOwner {
        if (provingAutoPauseEnabled == _enable) revert GP_INVALID_STATUS();
        provingAutoPauseEnabled = _enable;

        emit ProvingAutoPauseEnabled(_enable);
    }

    /// @notice Enables unlimited allowance for Taiko L1 contract.
    /// param _enable true if unlimited allowance is approved, false to set the allowance to 0.
    function enableTaikoTokenAllowance(bool _enable) external onlyOwner {
        address tko = resolve(LibStrings.B_TAIKO_TOKEN, false);
        address taiko = resolve(LibStrings.B_TAIKO, false);
        IERC20(tko).approve(taiko, _enable ? type(uint256).max : 0);
    }

    /// @dev Withdraws Taiko Token to a given address.
    /// @param _to The recipient address.
    /// @param _amount The amount of Taiko token to withdraw. Use 0 for all balance.
    function withdrawTaikoToken(address _to, uint256 _amount) external onlyOwner {
        if (_to == address(0)) revert GV_ZERO_ADDRESS();

        IERC20 tko = IERC20(resolve(LibStrings.B_TAIKO_TOKEN, false));
        uint256 amount = _amount == 0 ? tko.balanceOf(address(this)) : _amount;
        tko.transfer(_to, amount);
    }

    /// @dev Called by guardians to approve a guardian proof
    /// @param _meta The block's metadata.
    /// @param _tran The valid transition.
    /// @return approved_ True if the minimum number of approval is acquired, false otherwise.
    function approve(
        TaikoData.BlockMetadata calldata _meta,
        TaikoData.Transition calldata _tran /*,
        TaikoData.TierProof calldata _proof*/
    )
        external
        whenNotPaused
        nonReentrant
        returns (bool approved_)
    {
        bytes32 proofHash = keccak256(abi.encode(_meta, _tran, "")); //"" shall be removed
        uint256 _version = version;
        bytes32 currProofHash = latestProofHash[_version][0]; // constant ID for now.. for taiko-mon
            // vs. taiko simplified comp.

        if (currProofHash == 0) {
            latestProofHash[_version][0] = proofHash;
            currProofHash = proofHash;
        }

        bool conflicting = currProofHash != proofHash;
        bool pauseProving = conflicting && provingAutoPauseEnabled
            && address(this) == resolve(LibStrings.B_CHAIN_WATCHDOG, true);

        if (conflicting) {
            latestProofHash[_version][0] = proofHash;
            emit ConflictingProofs(0, msg.sender, currProofHash, proofHash, pauseProving);
        }

        if (pauseProving) {
            ITaikoL1(resolve(LibStrings.B_TAIKO, false)).pauseProving(true);
        } else {
            approved_ = _approve(0, proofHash);
            emit GuardianApproval(msg.sender, 0, _tran.blockHash, approved_, ""); // "" = empty
                // bytes

            if (approved_) {
                delete approvals[_version][proofHash];
                delete latestProofHash[_version][0];

                ITaikoL1(resolve(LibStrings.B_TAIKO, false)).proveBlock(
                    0,
                    abi.encode(_meta, _tran, "") // empty bytes as proof for taiko-simplified vs.
                        // taiko-mono comp
                );
            }
        }
    }

    /// @notice Pauses chain proving and verification.
    function pauseTaikoProving() external whenNotPaused {
        if (guardianIds[msg.sender] == 0) revert GP_INVALID_GUARDIAN();

        if (address(this) != resolve(LibStrings.B_CHAIN_WATCHDOG, true)) {
            revert GV_PERMISSION_DENIED();
        }

        ITaikoL1(resolve(LibStrings.B_TAIKO, false)).pauseProving(true);
    }

    function verifyProof(
        /*Context calldata _ctx,*/
        TaikoData.Transition calldata /*,
        TaikoData.TierProof calldata*/
    )
        external
        view
    {
        //if (_ctx.msgSender != address(this)) revert GV_PERMISSION_DENIED();
    }

    /// @notice Returns the number of guardians
    /// @return The number of guardians
    function numGuardians() public view returns (uint256) {
        return guardians.length;
    }

    function _approve(uint256 _blockId, bytes32 _proofHash) internal returns (bool approved_) {
        uint256 id = guardianIds[msg.sender];
        if (id == 0) revert GP_INVALID_GUARDIAN();

        uint256 _version = version;

        unchecked {
            approvals[_version][_proofHash] |= 1 << (id - 1);
        }

        uint256 _approval = approvals[_version][_proofHash];
        approved_ = _isApproved(_approval);
        emit Approved(_blockId, _approval, approved_);
    }

    function _isApproved(uint256 _approvalBits) private view returns (bool) {
        uint256 count;
        uint256 bits = _approvalBits;
        uint256 guardiansLength = guardians.length;
        unchecked {
            for (uint256 i; i < guardiansLength; ++i) {
                if (bits & 1 == 1) ++count;
                if (count == minGuardians) return true;
                bits >>= 1;
            }
        }
        return false;
    }
}
