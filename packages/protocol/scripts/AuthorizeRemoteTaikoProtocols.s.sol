// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "forge-std/console2.sol";
import "../contracts/signal/SignalService.sol";

contract AuthorizeRemoteTaikoProtocols is Script {
    uint256 public privateKey = vm.envUint("PRIVATE_KEY");
    address public signalServiceAddress = vm.envAddress("SIGNAL_SERVICE_ADDRESS");
    uint256[] public remoteChainIDs = vm.envUint("REMOTE_CHAIN_IDS", ",");
    address[] public remoteTaikoProtocols = vm.envAddress("REMOTE_TAIKO_PROTOCOLS", ",");

    function run() external {
        require(
            remoteChainIDs.length == remoteTaikoProtocols.length,
            "invalid remote taiko protocol addresses length"
        );

        vm.startBroadcast(privateKey);

        SignalService signalService = SignalService(payable(signalServiceAddress));
        for (uint256 i; i < remoteChainIDs.length; ++i) {
            console2.log(remoteTaikoProtocols[i], "--->", remoteChainIDs[i]);
            if (!signalService.isAuthorizedAs(remoteTaikoProtocols[i], bytes32(remoteChainIDs[i])))
            {
                signalService.authorize(remoteTaikoProtocols[i], bytes32(remoteChainIDs[i]));
            }
        }

        vm.stopBroadcast();
    }
}
