// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

import "lib/openzeppelin-contracts-upgradeable/contracts/token/ERC721/ERC721Upgradeable.sol";
import "./MerkleClaimable.sol";

/// @title ERC721Airdrop
contract ERC721Airdrop is MerkleClaimable {
    address public token;
    address public vault;
    uint256[48] private __gap;

    function init(
        uint64 _claimStarts,
        uint64 _claimEnds,
        bytes32 _merkleRoot,
        address _token,
        address _vault
    )
        external
        initializer
    {
        __Essential_init();
        _setConfig(_claimStarts, _claimEnds, _merkleRoot);

        token = _token;
        vault = _vault;
    }

    function _claimWithData(bytes calldata data) internal override {
        (address user, uint256[] memory tokenIds) = abi.decode(data, (address, uint256[]));

        for (uint256 i; i < tokenIds.length; ++i) {
            IERC721Upgradeable(token).safeTransferFrom(vault, user, tokenIds[i]);
        }
    }
}
