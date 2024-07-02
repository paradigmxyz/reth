// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

import "../test/DeployCapability.sol";
import "../contracts/L1/gov/TaikoTimelockController.sol";

contract SetRemoteBridgeSuites is DeployCapability {
    uint256 public privateKey = vm.envUint("PRIVATE_KEY");
    uint256 public securityCouncilPrivateKey = vm.envUint("SECURITY_COUNCIL_PRIVATE_KEY");
    address public timelockAddress = vm.envAddress("TIMELOCK_ADDRESS");
    address public addressManagerAddress = vm.envAddress("ADDRESS_MANAGER_ADDRESS");
    uint256[] public remoteChainIDs = vm.envUint("REMOTE_CHAIN_IDS", ",");
    address[] public remoteBridges = vm.envAddress("REMOTE_BRIDGES", ",");
    address[] public remoteERC20Vaults = vm.envAddress("REMOTE_ERC20_VAULTS", ",");
    address[] public remoteERC721Vaults = vm.envAddress("REMOTE_ERC721_VAULTS", ",");
    address[] public remoteERC1155Vaults = vm.envAddress("REMOTE_ERC1155_VAULTS", ",");

    function run() external {
        require(
            remoteChainIDs.length == remoteBridges.length, "invalid remote bridge addresses length"
        );
        require(
            remoteChainIDs.length == remoteERC20Vaults.length,
            "invalid remote ERC20Vault addresses length"
        );
        require(
            remoteChainIDs.length == remoteERC721Vaults.length,
            "invalid remote ERC721Vault addresses length"
        );
        require(
            remoteChainIDs.length == remoteERC1155Vaults.length,
            "invalid remote ERC1155Vault addresses length"
        );

        vm.startBroadcast(privateKey);

        for (uint256 i; i < remoteChainIDs.length; ++i) {
            uint64 chainid = uint64(remoteChainIDs[i]);

            if (securityCouncilPrivateKey == 0) {
                register(addressManagerAddress, "bridge", remoteBridges[i], chainid);
                register(addressManagerAddress, "erc20_vault", remoteERC20Vaults[i], chainid);
                register(addressManagerAddress, "erc721_vault", remoteERC721Vaults[i], chainid);
                register(addressManagerAddress, "erc1155_vault", remoteERC1155Vaults[i], chainid);
                continue;
            }

            registerByTimelock(addressManagerAddress, "bridge", remoteBridges[i], chainid);
            registerByTimelock(addressManagerAddress, "erc20_vault", remoteERC20Vaults[i], chainid);
            registerByTimelock(
                addressManagerAddress, "erc721_vault", remoteERC721Vaults[i], chainid
            );
            registerByTimelock(
                addressManagerAddress, "erc1155_vault", remoteERC1155Vaults[i], chainid
            );
        }

        vm.stopBroadcast();
    }

    function registerByTimelock(
        address registerTo,
        string memory name,
        address addr,
        uint64 chainId
    )
        internal
    {
        bytes32 salt = bytes32(block.timestamp);

        bytes memory payload = abi.encodeWithSelector(
            bytes4(keccak256("setAddress(uint64,bytes32,address)")),
            chainId,
            bytes32(bytes(name)),
            addr
        );

        TaikoTimelockController timelock = TaikoTimelockController(payable(timelockAddress));

        timelock.schedule(registerTo, 0, payload, bytes32(0), salt, 0);

        timelock.execute(registerTo, 0, payload, bytes32(0), salt);

        console2.log("> ", name, "@", registerTo);
        console2.log("\t addr : ", addr);
    }
}
