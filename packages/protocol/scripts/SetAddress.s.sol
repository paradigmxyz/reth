// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "forge-std/console2.sol";

import "../contracts/common/AddressManager.sol";

contract SetAddress is Script {
    uint256 public adminPrivateKey = vm.envUint("PRIVATE_KEY");

    address public proxyAddress = vm.envAddress("PROXY_ADDRESS");

    uint64 public domain = uint64(vm.envUint("DOMAIN"));

    bytes32 public name = vm.envBytes32("NAME");

    address public addr = vm.envAddress("ADDRESS");

    AddressManager proxy;

    function run() external {
        require(adminPrivateKey != 0, "PRIVATE_KEY not set");
        require(proxyAddress != address(0), "PROXY_ADDRESS not set");
        require(domain != 0, "DOMAIN NOT SET");
        require(name != bytes32(0), "NAME NOT SET");
        require(addr != address(0), "ADDR NOT SET");

        vm.startBroadcast(adminPrivateKey);

        proxy = AddressManager(payable(proxyAddress));

        proxy.setAddress(domain, name, addr);

        vm.stopBroadcast();
    }
}
