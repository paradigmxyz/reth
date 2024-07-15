// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import "../TaikoTest.sol";

contract TestBridgedERC20 is TaikoTest {
    address manager;
    address vault = randAddress();
    address owner = randAddress();

    function setUp() public {
        manager = deployProxy({
            name: "address_manager",
            impl: address(new AddressManager()),
            data: abi.encodeCall(AddressManager.init, (address(0)))
        });

        register(manager, "erc20_vault", vault);
    }

    function test_20Vault_migration__change_migration_status() public {
        BridgedERC20 btoken = deployBridgedToken("FOO");

        vm.expectRevert();
        btoken.changeMigrationStatus(Emma, false);

        vm.startPrank(vault);
        btoken.changeMigrationStatus(Frank, false);
        btoken.changeMigrationStatus(address(0), false);
        btoken.changeMigrationStatus(address(0), true);
        btoken.changeMigrationStatus(Frank, true);
        vm.expectRevert();
        btoken.changeMigrationStatus(Frank, true);

        vm.stopPrank();
    }

    function test_20Vault_migration___only_vault_can_min__but_cannot_burn_when_migration_off()
        public
    {
        BridgedERC20 btoken = deployBridgedToken("BAR");
        // only erc20_vault can brun and mint
        vm.prank(vault, vault);
        btoken.mint(Bob, 1000);
        //Vault cannot burn only if it owns the tokens
        vm.expectRevert();
        vm.prank(Bob, Bob);
        btoken.burn(600);
        assertEq(btoken.balanceOf(Bob), 1000);
        vm.stopPrank();

        // Owner can burn/mint
        vm.prank(owner, owner);
        btoken.mint(Bob, 1000);
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

        // but can be done by the token owner - if migrating out phase
        vm.prank(Bob);
        oldToken.burn(10);
        assertEq(oldToken.balanceOf(Bob), 90);
        assertEq(newToken.balanceOf(Bob), 210);

        // Testing newToken
        // 1. Nobody can mint except the vault
        vm.prank(Bob);
        vm.expectRevert();
        newToken.mint(Bob, 10);

        vm.prank(owner);
        newToken.mint(Bob, 10);

        vm.prank(vault);
        newToken.mint(Bob, 15);
        assertEq(newToken.balanceOf(Bob), 235);

        // Vault can only burn if it owns the tokens
        vm.prank(vault);
        vm.expectRevert();
        newToken.burn(25);
        assertEq(newToken.balanceOf(Bob), 235);

        // Imitate current bridge-back operation, as Bob gave approval (for bridging back) and then
        // ERC20Vault does the "transfer and burn"
        vm.prank(Bob);
        newToken.approve(vault, 25);

        // Following the "transfer and burn" pattern
        vm.prank(vault);
        newToken.transferFrom(Bob, vault, 25);

        vm.prank(vault);
        newToken.burn(25);

        assertEq(newToken.balanceOf(Bob), 210);
    }

    function deployBridgedToken(string memory name) internal returns (BridgedERC20) {
        address srcToken = randAddress();
        uint256 srcChainId = 1000;
        uint8 srcDecimals = 11;
        return BridgedERC20(
            deployProxy({
                name: "bridged_token1",
                impl: address(new BridgedERC20()),
                data: abi.encodeCall(
                    BridgedERC20.init,
                    (owner, address(manager), srcToken, srcChainId, srcDecimals, name, name)
                ),
                registerTo: manager
            })
        );
    }
}
