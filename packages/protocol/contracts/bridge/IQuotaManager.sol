// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

/// @title IQuotaManager
/// @custom:security-contact security@taiko.xyz
interface IQuotaManager {
    /// @notice Consumes a specific amount of quota for a given address.
    /// This function must revert if available quota is smaller than the given amount of quota.
    ///
    /// @dev Note that IQuotaManager is used by vaults and bridge, and should be registered in a
    /// shared address manager on the L1, therefore, a registered IQuotaManager and its per-token
    /// quota settings will be shared by all Taiko L2s. To enable a per-L2 quota, we need to modify
    /// this function to:
    ///  `function consumeQuota(uint256 _srcChainId, address _token, uint256 _amount) `
    ///
    /// @param _token The token address. Ether is represented with address(0).
    /// @param _amount The amount of quota to consume.
    function consumeQuota(address _token, uint256 _amount) external;
}
