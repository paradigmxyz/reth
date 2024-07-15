// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

import "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import "../../common/AddressResolver.sol";
import "../../libs/LibAddress.sol";
import "../TaikoData.sol";
import "./ProverPayment.sol";

/// @title LibProposing
/// @notice A library for handling block proposals in the Taiko protocol.
contract PBSActor {
    using LibAddress for address;

    ProverPayment public operator;

    /// @dev Proposes a Taiko L2 block.
    function proposeBlock(
        bytes[] calldata data,
        bytes[] calldata txLists,
        bytes memory proverPaymentData,
        uint256 tip
    )
        external
        payable
    {
        // TODO(Brecht): just pass in opaque data to make it general, though kind of doesn't matter
       operator.proposeBlock{ value: msg.value - tip }(data, txLists, proverPaymentData);

        // Do conditional payment
        address(block.coinbase).sendEtherAndVerify(tip);
    }
}
