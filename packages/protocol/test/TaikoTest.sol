// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "forge-std/console2.sol";

import "@openzeppelin/contracts/proxy/transparent/TransparentUpgradeableProxy.sol";

import "../contracts/thirdparty/LibFixedPointMath.sol";

import "../contracts/bridge/Bridge.sol";
import "../contracts/signal/SignalService.sol";
import "../contracts/tokenvault/BridgedERC20.sol";
import "../contracts/tokenvault/BridgedERC721.sol";
import "../contracts/tokenvault/BridgedERC1155.sol";
import "../contracts/tokenvault/ERC20Vault.sol";
import "../contracts/tokenvault/ERC721Vault.sol";
import "../contracts/tokenvault/ERC1155Vault.sol";

import "../contracts/L1/TaikoToken.sol";
import "../contracts/L1/BasedOperator.sol";
import "../contracts/L1/VerifierRegistry.sol";
import "../contracts/L1/verifiers/SgxVerifier.sol";
/*import "../contracts/L1/TaikoL1.sol";
import "../contracts/L1/verifiers/GuardianVerifier.sol";
import "../contracts/L1/verifiers/PseZkVerifier.sol";
import "../contracts/L1/verifiers/SgxAndZkVerifier.sol";
import "../contracts/L1/tiers/TaikoA6TierProvider.sol";
import "../contracts/L1/tiers/ITierProvider.sol";
import "../contracts/L1/tiers/ITierProvider.sol";
import "../contracts/L1/provers/GuardianProver.sol";*/

import "../contracts/L2/Lib1559Math.sol";
import "../contracts/L2/TaikoL2EIP1559Configurable.sol";
import "../contracts/L2/TaikoL2.sol";

import "../contracts/team/TimelockTokenPool.sol";
import "../contracts/team/airdrop/ERC20Airdrop.sol";
import "../contracts/team/airdrop/ERC20Airdrop2.sol";

import "../contracts/test/erc20/FreeMintERC20.sol";

import "./DeployCapability.sol";
import "./HelperContracts.sol";

abstract contract TaikoTest is Test, DeployCapability {
    uint256 private _seed = 0x12345678;
    address internal Alice = vm.addr(0x1);
    address internal Bob = vm.addr(0x2);
    address internal Carol = vm.addr(0x3);
    address internal David = randAddress();
    address internal Emma = randAddress();
    address internal Frank = randAddress();
    address internal Grace = randAddress();
    address internal Henry = randAddress();
    address internal Isabella = randAddress();
    address internal James = randAddress();
    address internal Katherine = randAddress();
    address internal Liam = randAddress();
    address internal Mia = randAddress();
    address internal Noah = randAddress();
    address internal Olivia = randAddress();
    address internal Patrick = randAddress();
    address internal Quinn = randAddress();
    address internal Rachel = randAddress();
    address internal Samuel = randAddress();
    address internal Taylor = randAddress();
    address internal Ulysses = randAddress();
    address internal Victoria = randAddress();
    address internal William = randAddress();
    address internal Xavier = randAddress();
    address internal Yasmine = randAddress();
    address internal Zachary = randAddress();
    address internal SGX_X_0 = vm.addr(0x4);
    address internal SGX_X_1 = vm.addr(0x5);
    address internal SGX_Y = randAddress();
    address internal SGX_Z = randAddress();

    function randAddress() internal returns (address) {
        bytes32 randomHash = keccak256(abi.encodePacked("address", _seed++));
        return address(bytes20(randomHash));
    }

    function randBytes32() internal returns (bytes32) {
        return keccak256(abi.encodePacked("bytes32", _seed++));
    }
}
