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

    // This tests shows that the admin() and owner() cannot be the same, otherwise,
    // the owner cannot transact delegated functions on implementation.
    function test_essential_behind_transparent_proxy() external {
        bytes memory data = bytes.concat(Target1.init.selector);
        vm.startPrank(Alice);
        TransparentUpgradeableProxy proxy =
            new TransparentUpgradeableProxy(address(new Target1()), Bob, data);
        Target1 target = Target1(address(proxy));
        vm.stopPrank();

        // Owner is Alice
        // Admin is Bob

        vm.prank(Carol);
        assertEq(target.owner(), Alice);

        // Only Bob can call admin()
        vm.prank(Bob);
        assertEq(proxy.admin(), Bob);

        // Other people, including Alice, cannot call admin()
        vm.prank(Alice);
        vm.expectRevert();
        proxy.admin();

        vm.prank(Carol);
        vm.expectRevert();
        proxy.admin();

        // Alice can adjust();
        vm.prank(Alice);
        target.adjust();
        assertEq(target.count(), 101);

        // Bob cannot adjust()
        vm.prank(Bob);
        vm.expectRevert();
        target.adjust();

        // Transfer Owner to Bob, so Bob is both admin and owner
        vm.prank(Alice);
        target.transferOwnership(Bob);

        vm.prank(Carol);
        assertEq(target.owner(), Bob);

        // Now Bob cannot call adjust()
        vm.prank(Bob);
        vm.expectRevert();
        target.adjust();
    }
}
