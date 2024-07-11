// SPDX-License-Identifier: MIT

pragma solidity 0.8.24;

import "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";
import "@openzeppelin/contracts-upgradeable/access/OwnableUpgradeable.sol";

import "forge-std/console2.sol";
import "forge-std/Script.sol";

import "../contracts/common/AddressManager.sol";

/// @title DeployCapability
abstract contract DeployCapability is Script {
    error ADDRESS_NULL();

    function deployProxy(
        string memory name,
        address impl,
        bytes memory data,
        address registerTo
    )
        internal
        returns (address proxy)
    {
        proxy = address(new ERC1967Proxy(impl, data));

        if (registerTo != address(0)) {
            AddressManager(registerTo).setAddress(
                uint64(block.chainid), bytes32(bytes(name)), proxy
            );
        }

        console2.log(">", name, "@", registerTo);
        console2.log("  proxy      :", proxy);
        console2.log("  impl       :", impl);
        console2.log("  owner      :", OwnableUpgradeable(proxy).owner());
        console2.log("  msg.sender :", msg.sender);
        console2.log("  this       :", address(this));

        vm.writeJson(
            vm.serializeAddress("deployment", name, proxy),
            string.concat(vm.projectRoot(), "/deployments/deploy_l1.json")
        );
    }

    function deployProxy(
        string memory name,
        address impl,
        bytes memory data
    )
        internal
        returns (address proxy)
    {
        return deployProxy(name, impl, data, address(0));
    }

    function register(address registerTo, string memory name, address addr) internal {
        register(registerTo, name, addr, uint64(block.chainid));
    }

    function register(
        address registerTo,
        string memory name,
        address addr,
        uint64 chainId
    )
        internal
    {
        if (registerTo == address(0)) revert ADDRESS_NULL();
        if (addr == address(0)) revert ADDRESS_NULL();
        AddressManager(registerTo).setAddress(chainId, bytes32(bytes(name)), addr);
        console2.log("> ", name, "@", registerTo);
        console2.log("\t addr : ", addr);
    }

    function copyRegister(address registerTo, address readFrom, string memory name) internal {
        if (registerTo == address(0)) revert ADDRESS_NULL();
        if (readFrom == address(0)) revert ADDRESS_NULL();

        register({
            registerTo: registerTo,
            name: name,
            addr: AddressManager(readFrom).getAddress(uint64(block.chainid), bytes32(bytes(name))),
            chainId: uint64(block.chainid)
        });
    }
}
