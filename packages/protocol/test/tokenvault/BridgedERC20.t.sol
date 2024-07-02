// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "../TaikoTest.sol";

contract TestBridgedERC20 is TaikoTest {
    address manager;
    address vault = randAddress();
    address owner = randAddress();

    function setUp() public {
        manager = deployProxy({
            name: "address_manager",
            impl: address(new AddressManager()),
            data: bytes.concat(AddressManager.init.selector)
        });

        register(manager, "erc20_vault", vault);
    }

    function test_20Vault_migration__change_migration_status() public {
        BridgedERC20 btoken = deployBridgedToken("FOO");

        vm.expectRevert();
        btoken.changeMigrationStatus(Emma, false);

        vm.startPrank(owner);
        btoken.changeMigrationStatus(Emma, false);
        btoken.changeMigrationStatus(address(0), false);
        btoken.changeMigrationStatus(address(0), true);
        btoken.changeMigrationStatus(Emma, true);
        vm.expectRevert();
        btoken.changeMigrationStatus(Emma, true);
        vm.stopPrank();

        vm.startPrank(vault);
        btoken.changeMigrationStatus(Frank, false);
        btoken.changeMigrationStatus(address(0), false);
        btoken.changeMigrationStatus(address(0), true);
        btoken.changeMigrationStatus(Frank, true);
        vm.expectRevert();
        btoken.changeMigrationStatus(Frank, true);

        vm.stopPrank();
    }

    function test_20Vault_migration___only_vault_can_min_burn_when_migration_off() public {
        BridgedERC20 btoken = deployBridgedToken("BAR");
        // only erc20_vault can brun and mint
        vm.startPrank(vault);
        btoken.mint(Bob, 1000);
        btoken.burn(Bob, 600);
        assertEq(btoken.balanceOf(Bob), 400);
        vm.stopPrank();

        // Owner cannot burn/mint
        vm.startPrank(owner);
        vm.expectRevert();
        btoken.mint(Bob, 1000);
        vm.expectRevert();
        btoken.burn(Bob, 100);
        vm.stopPrank();
    }

    function test_20Vault_migration__old_to_new() public {
        BridgedERC20 oldToken = deployBridgedToken("OLD");
        BridgedERC20 newToken = deployBridgedToken("NEW");

        vm.startPrank(vault);
        oldToken.mint(Bob, 100);
        newToken.mint(Bob, 200);

        oldToken.changeMigrationStatus(address(newToken), false);
        newToken.changeMigrationStatus(address(oldToken), true);
        vm.stopPrank();

        // Testing oldToken
        // 1. minting is not possible for Bob, owner, or vault
        vm.prank(Bob);
        vm.expectRevert();
        oldToken.mint(Bob, 10);

        vm.prank(owner);
        vm.expectRevert();
        oldToken.mint(Bob, 10);

        vm.prank(vault);
        vm.expectRevert();
        oldToken.mint(Bob, 10);

        // 2. burning can be done by anyone
        vm.prank(randAddress());
        oldToken.burn(Bob, 10);
        assertEq(oldToken.balanceOf(Bob), 90);
        assertEq(newToken.balanceOf(Bob), 210);

        // Testing newToken
        // 1. Nobody can mint except the vault
        vm.prank(Bob);
        vm.expectRevert();
        newToken.mint(Bob, 10);

        vm.prank(owner);
        vm.expectRevert();
        newToken.mint(Bob, 10);

        vm.prank(vault);
        newToken.mint(Bob, 15);
        assertEq(newToken.balanceOf(Bob), 225);

        // 2. Nobody can burn except the vault
        vm.prank(Bob);
        vm.expectRevert();
        newToken.burn(Bob, 10);

        vm.prank(owner);
        vm.expectRevert();
        newToken.burn(Bob, 10);

        vm.prank(vault);
        newToken.burn(Bob, 25);
        assertEq(newToken.balanceOf(Bob), 200);
    }

    function deployBridgedToken(string memory name) internal returns (BridgedERC20) {
        address srcToken = randAddress();
        uint256 srcChainId = 1000;
        uint8 srcDecimals = 11;
        return BridgedERC20(
            deployProxy({
                name: "bridged_token1",
                impl: address(new BridgedERC20()),
                data: bytes.concat(
                    BridgedERC20.init.selector,
                    abi.encode(address(manager), srcToken, srcChainId, srcDecimals, name, name)
                    ),
                registerTo: manager,
                owner: owner
            })
        );
    }
}
