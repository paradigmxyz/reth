// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import "../common/EssentialContract.sol";
import "../common/LibStrings.sol";
import "../libs/LibMath.sol";
import "./IQuotaManager.sol";

/// @title QuotaManager
/// @dev An implementation of IQuotaManager for Ether and ERC20 tokens.
/// @custom:security-contact security@taiko.xyz
contract QuotaManager is EssentialContract, IQuotaManager {
    using LibMath for uint256;

    struct Quota {
        uint48 updatedAt;
        uint104 quota;
        uint104 available;
    }

    mapping(address token => Quota tokenLimit) public tokenQuota;
    uint24 public quotaPeriod;

    uint256[48] private __gap;

    event QuotaUpdated(address indexed token, uint256 oldQuota, uint256 newQuota);
    event QuotaPeriodUpdated(uint256 quotaPeriod);

    error QM_INVALID_PARAM();
    error QM_OUT_OF_QUOTA();

    /// @notice Initializes the contract.
    /// @param _owner The owner of this contract. msg.sender will be used if this value is zero.
    /// @param _addressManager The address of the {AddressManager} contract.
    /// @param _quotaPeriod The time required to restore all quota.
    function init(
        address _owner,
        address _addressManager,
        uint24 _quotaPeriod
    )
        external
        initializer
    {
        __Essential_init(_owner, _addressManager);
        _setQuotaPeriod(_quotaPeriod);
    }

    /// @notice Updates the daily quota for a given address.
    /// @param _token The token address with Ether represented by address(0).
    /// @param _quota The new quota for the defined period.
    function updateQuota(address _token, uint104 _quota) external onlyOwner whenNotPaused {
        emit QuotaUpdated(_token, tokenQuota[_token].quota, _quota);
        tokenQuota[_token] = Quota(0, _quota, _quota);
    }

    function setQuotaPeriod(uint24 _quotaPeriod) external onlyOwner whenNotPaused {
        _setQuotaPeriod(_quotaPeriod);
    }

    /// @inheritdoc IQuotaManager
    function consumeQuota(
        address _token,
        uint256 _amount
    )
        external
        whenNotPaused
        onlyFromNamedEither(LibStrings.B_BRIDGE, LibStrings.B_ERC20_VAULT)
    {
        uint256 available = availableQuota(_token, 0);
        if (available == type(uint256).max) return;
        if (available < _amount) revert QM_OUT_OF_QUOTA();

        unchecked {
            available -= _amount;
        }
        tokenQuota[_token].available = uint104(available);
        tokenQuota[_token].updatedAt = uint48(block.timestamp);
    }

    /// @notice Returns the available quota for a given token.
    /// @param _token The token address with Ether represented by address(0).
    /// @param _leap Amount of seconds in the future.
    /// @return The available quota.
    function availableQuota(address _token, uint256 _leap) public view returns (uint256) {
        Quota memory q = tokenQuota[_token];
        if (q.quota == 0) return type(uint256).max;
        if (q.updatedAt == 0) return q.quota;

        uint256 issuance = q.quota * (block.timestamp + _leap - q.updatedAt) / quotaPeriod;
        return (issuance + q.available).min(q.quota);
    }

    function _setQuotaPeriod(uint24 _quotaPeriod) private {
        if (_quotaPeriod == 0) revert QM_INVALID_PARAM();
        quotaPeriod = _quotaPeriod;
        emit QuotaPeriodUpdated(_quotaPeriod);
    }
}
