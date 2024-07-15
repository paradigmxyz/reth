// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "./TaikoL1TestBase.sol";
/*
contract TestSgxVerifier is TaikoL1TestBase {
    function deployTaikoL1() internal override returns (TaikoL1) {
        return
TaikoL1(payable(deployProxy({ name: "taiko", impl: address(new TaikoL1()), data: "" })));
    }

    function test_addInstancesByOwner() external {
        address[] memory _instances = new address[](3);
        _instances[0] = SGX_X_1;
        _instances[1] = SGX_Y;
        _instances[2] = SGX_Z;
        sv.addInstances(_instances);
    }

    function test_addInstancesByOwner_WithoutOwnerRole() external {
        address[] memory _instances = new address[](3);
        _instances[0] = SGX_X_0;
        _instances[1] = SGX_Y;
        _instances[2] = SGX_Z;

        vm.expectRevert();
        vm.prank(Bob, Bob);
        sv.addInstances(_instances);
    }

    function test_addInstancesBySgxInstance() external {
        address[] memory _instances = new address[](2);
        _instances[0] = SGX_Y;
        _instances[1] = SGX_Z;

        bytes memory signature = _getSignature(_instances, 0x4);

        vm.prank(Bob, Bob);
        sv.addInstances(0, SGX_X_1, _instances, signature);
    }

    function _getSignature(
        address[] memory _instances,
        uint256 privKey
    )
        private
        pure
        returns (bytes memory signature)
    {
        bytes32 digest = keccak256(abi.encode("ADD_INSTANCES", _instances));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(privKey, digest);
        signature = abi.encodePacked(r, s, v);
    }
}
*/
