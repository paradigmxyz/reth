// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "../TaikoTest.sol";

contract Target1 is EssentialContract {
    uint256 public count;

    function init() external initializer {
        __Essential_init();
        count = 100;
    }

    function adjust() external virtual onlyOwner {
        count += 1;
    }
}

contract Target2 is Target1 {
    function update() external onlyOwner {
        count += 10;
    }

    function adjust() external override onlyOwner {
        count -= 1;
    }
}

contract TestOwnerUUPSUpgradable is TaikoTest {
    function test_essential_behind_1967_proxy() external {
        bytes memory data = bytes.concat(Target1.init.selector);
        vm.startPrank(Alice);
        ERC1967Proxy proxy = new ERC1967Proxy(address(new Target1()), data);
        Target1 target = Target1(address(proxy));
        vm.stopPrank();

        // Owner is Alice
        vm.prank(Carol);
        assertEq(target.owner(), Alice);

        // Alice can adjust();
        vm.prank(Alice);
        target.adjust();
        assertEq(target.count(), 101);

        // Bob cannot adjust()
        vm.prank(Bob);
        vm.expectRevert();
        target.adjust();

        address v2 = address(new Target2());
        data = bytes.concat(Target2.update.selector);

        vm.prank(Bob);
        vm.expectRevert();
        target.upgradeToAndCall(v2, data);

        vm.prank(Alice);
        target.upgradeToAndCall(v2, data);
        assertEq(target.count(), 111);

        vm.prank(Alice);
        target.adjust();
        assertEq(target.count(), 110);
    }
}
