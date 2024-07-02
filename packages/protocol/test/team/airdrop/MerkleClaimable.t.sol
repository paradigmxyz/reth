// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "../../TaikoTest.sol";

contract MyERC20 is ERC20 {
    constructor(address owner) ERC20("Taiko Token", "TKO") {
        _mint(owner, 1_000_000_000e18);
    }
}

contract TestERC20Airdrop is TaikoTest {
    uint64 claimStart;
    uint64 claimEnd;
    address internal owner = randAddress();

    bytes32 merkleRoot = 0x73a7330a8657ad864b954215a8f636bb3709d2edea60bcd4fcb8a448dbc6d70f;

    ERC20Airdrop airdrop;
    ERC20Airdrop2 airdrop2;
    ERC20 token;

    function setUp() public {
        token = new MyERC20(address(owner));
        // 1st 'genesis' airdrop
        airdrop = ERC20Airdrop(
            deployProxy({
                name: "airdrop",
                impl: address(new ERC20Airdrop()),
                data: bytes.concat(
                    ERC20Airdrop.init.selector, abi.encode(0, 0, merkleRoot, address(token), owner)
                    )
            })
        );

        // 2nd airdrop subject to unlocking (e.g. 10 days after starting after
        // claim window)
        airdrop2 = ERC20Airdrop2(
            deployProxy({
                name: "airdrop",
                impl: address(new ERC20Airdrop2()),
                data: bytes.concat(
                    ERC20Airdrop2.init.selector,
                    abi.encode(0, 0, merkleRoot, address(token), owner, 10 days)
                    )
            })
        );

        claimStart = uint64(block.timestamp + 10);
        claimEnd = uint64(block.timestamp + 10_000);

        airdrop.setConfig(claimStart, claimEnd, merkleRoot);

        airdrop2.setConfig(claimStart, claimEnd, merkleRoot);

        vm.roll(block.number + 1);
        vm.warp(block.timestamp + 12);

        vm.prank(owner, owner);
        MyERC20(address(token)).approve(address(airdrop), 1_000_000_000e18);

        vm.prank(owner, owner);
        MyERC20(address(token)).approve(address(airdrop2), 1_000_000_000e18);
    }

    function test_claim_but_claim_not_ongoing_yet() public {
        vm.warp(1);
        bytes32[] memory merkleProof = new bytes32[](3);
        merkleProof[0] = 0x4014b456db813d18e801fe3b30bbe14542c9c84caa9a92b643f7f46849283077;
        merkleProof[1] = 0xfc2f09b34fb9437f9bde16049237a2ab3caa6d772bd794da57a8c314aea22b3f;
        merkleProof[2] = 0xc13844b93533d8aec9c7c86a3d9399efb4e834f4069b9fd8a88e7290be612d05;

        vm.expectRevert(MerkleClaimable.CLAIM_NOT_ONGOING.selector);
        vm.prank(Alice, Alice);
        airdrop.claim(abi.encode(Alice, 100), merkleProof);
    }

    function test_claim_but_claim_not_ongoing_anymore() public {
        vm.warp(uint64(block.timestamp + 11_000));

        bytes32[] memory merkleProof = new bytes32[](3);
        merkleProof[0] = 0x4014b456db813d18e801fe3b30bbe14542c9c84caa9a92b643f7f46849283077;
        merkleProof[1] = 0xfc2f09b34fb9437f9bde16049237a2ab3caa6d772bd794da57a8c314aea22b3f;
        merkleProof[2] = 0xc13844b93533d8aec9c7c86a3d9399efb4e834f4069b9fd8a88e7290be612d05;

        vm.expectRevert(MerkleClaimable.CLAIM_NOT_ONGOING.selector);
        vm.prank(Alice, Alice);
        airdrop.claim(abi.encode(Alice, 100), merkleProof);
    }

    function test_claim_but_with_invalid_allowance() public {
        vm.warp(uint64(block.timestamp + 11));
        // These proofs are coming from 'pnpm run buildMerkle'
        bytes32[] memory merkleProof = new bytes32[](3);
        merkleProof[0] = 0x4014b456db813d18e801fe3b30bbe14542c9c84caa9a92b643f7f46849283077;
        merkleProof[1] = 0xfc2f09b34fb9437f9bde16049237a2ab3caa6d772bd794da57a8c314aea22b3f;
        merkleProof[2] = 0xc13844b93533d8aec9c7c86a3d9399efb4e834f4069b9fd8a88e7290be612d05;

        vm.expectRevert(MerkleClaimable.INVALID_PROOF.selector);
        vm.prank(Alice, Alice);
        airdrop.claim(abi.encode(Alice, 200), merkleProof);
    }

    function test_claim() public {
        vm.warp(uint64(block.timestamp + 11));
        // These proofs are coming from 'pnpm run buildMerkle'
        bytes32[] memory merkleProof = new bytes32[](3);
        merkleProof[0] = 0x4014b456db813d18e801fe3b30bbe14542c9c84caa9a92b643f7f46849283077;
        merkleProof[1] = 0xfc2f09b34fb9437f9bde16049237a2ab3caa6d772bd794da57a8c314aea22b3f;
        merkleProof[2] = 0xc13844b93533d8aec9c7c86a3d9399efb4e834f4069b9fd8a88e7290be612d05;

        vm.prank(Alice, Alice);
        airdrop.claim(abi.encode(Alice, 100), merkleProof);

        // Check Alice balance
        assertEq(token.balanceOf(Alice), 100);
    }

    function test_claim_with_same_proofs_twice() public {
        vm.warp(uint64(block.timestamp + 11));
        // These proofs are coming from 'pnpm run buildMerkle'
        bytes32[] memory merkleProof = new bytes32[](3);
        merkleProof[0] = 0x4014b456db813d18e801fe3b30bbe14542c9c84caa9a92b643f7f46849283077;
        merkleProof[1] = 0xfc2f09b34fb9437f9bde16049237a2ab3caa6d772bd794da57a8c314aea22b3f;
        merkleProof[2] = 0xc13844b93533d8aec9c7c86a3d9399efb4e834f4069b9fd8a88e7290be612d05;

        vm.prank(Alice, Alice);
        airdrop.claim(abi.encode(Alice, 100), merkleProof);

        // Check Alice balance
        assertEq(token.balanceOf(Alice), 100);

        vm.expectRevert(MerkleClaimable.CLAIMED_ALREADY.selector);
        vm.prank(Alice, Alice);
        airdrop.claim(abi.encode(Alice, 100), merkleProof);
    }

    function test_withdraw_for_airdrop2_withdraw_daily() public {
        vm.warp(uint64(block.timestamp + 11));
        // These proofs are coming from 'pnpm run buildMerkle'
        bytes32[] memory merkleProof = new bytes32[](3);
        merkleProof[0] = 0x4014b456db813d18e801fe3b30bbe14542c9c84caa9a92b643f7f46849283077;
        merkleProof[1] = 0xfc2f09b34fb9437f9bde16049237a2ab3caa6d772bd794da57a8c314aea22b3f;
        merkleProof[2] = 0xc13844b93533d8aec9c7c86a3d9399efb4e834f4069b9fd8a88e7290be612d05;

        vm.prank(Alice, Alice);
        airdrop2.claim(abi.encode(Alice, 100), merkleProof);

        // Try withdraw but not started yet
        vm.expectRevert(ERC20Airdrop2.WITHDRAWALS_NOT_ONGOING.selector);
        airdrop2.withdraw(Alice);

        // Roll one day after another, for 10 days and see the 100 allowance be withdrawn all and no
        // more left for the 11th day
        uint256 i = 1;
        uint256 balance;
        uint256 withdrawable;
        for (i = 1; i < 11; i++) {
            vm.roll(block.number + 200);
            vm.warp(claimEnd + (i * 1 days));

            (balance, withdrawable) = airdrop2.getBalance(Alice);

            assertEq(balance, 100);
            assertEq(withdrawable, 10);

            airdrop2.withdraw(Alice);
            // Check Alice balance
            assertEq(token.balanceOf(Alice), (i * 10));
        }

        // On the 10th day (midnight), Alice has no claims left
        vm.roll(block.number + 200);
        vm.warp(claimEnd + (10 days));

        (balance, withdrawable) = airdrop2.getBalance(Alice);

        assertEq(balance, 100);
        assertEq(withdrawable, 0);

        // No effect
        airdrop2.withdraw(Alice);
        // Check Alice balance
        assertEq(token.balanceOf(Alice), 100);
    }

    function test_withdraw_for_airdrop2_withdraw_at_the_end() public {
        vm.warp(uint64(block.timestamp + 11));
        // These proofs are coming from 'pnpm run buildMerkle'
        bytes32[] memory merkleProof = new bytes32[](3);
        merkleProof[0] = 0x4014b456db813d18e801fe3b30bbe14542c9c84caa9a92b643f7f46849283077;
        merkleProof[1] = 0xfc2f09b34fb9437f9bde16049237a2ab3caa6d772bd794da57a8c314aea22b3f;
        merkleProof[2] = 0xc13844b93533d8aec9c7c86a3d9399efb4e834f4069b9fd8a88e7290be612d05;

        vm.prank(Alice, Alice);
        airdrop2.claim(abi.encode(Alice, 100), merkleProof);

        // Try withdraw but not started yet
        vm.expectRevert(ERC20Airdrop2.WITHDRAWALS_NOT_ONGOING.selector);
        airdrop2.withdraw(Alice);

        // Roll 10 day after
        vm.roll(block.number + 200);
        vm.warp(claimEnd + 10 days);

        (uint256 balance, uint256 withdrawable) = airdrop2.getBalance(Alice);

        assertEq(balance, 100);
        assertEq(withdrawable, 100);

        airdrop2.withdraw(Alice);

        // Check Alice balance
        assertEq(token.balanceOf(Alice), 100);
    }

    function test_withdraw_for_airdrop2_but_out_of_withdrawal_window() public {
        vm.warp(uint64(block.timestamp + 11));
        // These proofs are coming from 'pnpm run buildMerkle'
        bytes32[] memory merkleProof = new bytes32[](3);
        merkleProof[0] = 0x4014b456db813d18e801fe3b30bbe14542c9c84caa9a92b643f7f46849283077;
        merkleProof[1] = 0xfc2f09b34fb9437f9bde16049237a2ab3caa6d772bd794da57a8c314aea22b3f;
        merkleProof[2] = 0xc13844b93533d8aec9c7c86a3d9399efb4e834f4069b9fd8a88e7290be612d05;

        vm.prank(Alice, Alice);
        airdrop2.claim(abi.encode(Alice, 100), merkleProof);

        // Try withdraw but not started yet
        vm.expectRevert(ERC20Airdrop2.WITHDRAWALS_NOT_ONGOING.selector);
        airdrop2.withdraw(Alice);

        // Roll 11 day after
        vm.roll(block.number + 200);
        vm.warp(claimEnd + 11 days);

        (uint256 balance, uint256 withdrawable) = airdrop2.getBalance(Alice);

        // Balance and withdrawable is 100,100 --> bc. it is out of withdrawal window
        assertEq(balance, 100);
        assertEq(withdrawable, 100);

        vm.expectRevert(ERC20Airdrop2.WITHDRAWALS_NOT_ONGOING.selector);
        airdrop2.withdraw(Alice);

        // Check Alice balance
        assertEq(token.balanceOf(Alice), 0);
    }
}
