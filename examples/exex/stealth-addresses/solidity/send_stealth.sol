// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import "forge-std/Script.sol";

interface IERC5564Announcer {
    function announce(
        uint256 schemeId,
        address stealthAddress,
        bytes memory ephemeralPubKey,
        bytes memory metadata
    ) external;
}

contract Stealthy is Script {

    function run(address stealthAddress, bytes memory ephemeralPubKey, bytes memory metadata) external payable {
        require(ephemeralPubKey.length == 33, "Public key must be 33 bytes");

        uint256 spender = vm.envUint("SPEND");
        IERC5564Announcer announcer = IERC5564Announcer(0x55649E01B5Df198D18D95b5cc5051630cfD45564);

        vm.startBroadcast(spender);
 
        payable(stealthAddress).transfer(0.5 ether);

        announcer.announce(
            1, // secp256k1
            stealthAddress,
            ephemeralPubKey,
            abi.encodePacked(metadata)
        );

        vm.stopBroadcast();

    }
}
