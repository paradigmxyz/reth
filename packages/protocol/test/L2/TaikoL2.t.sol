// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "../TaikoTest.sol";

contract SkipBasefeeCheckL2 is TaikoL2EIP1559Configurable {
    function skipFeeCheck() public pure override returns (bool) {
        return true;
    }
}

contract TestTaikoL2 is TaikoTest {
    using SafeCast for uint256;

    // Initial salt for semi-random generation
    uint256 salt = 2_195_684_615_435_261_315_311;
    // same as `block_gas_limit` in foundry.toml
    uint32 public constant BLOCK_GAS_LIMIT = 30_000_000;

    AddressManager public addressManager;
    SignalService public ss;
    TaikoL2EIP1559Configurable public L2;
    SkipBasefeeCheckL2 public L2skip;

    function setUp() public {
        addressManager = AddressManager(
            deployProxy({
                name: "address_manager",
                impl: address(new AddressManager()),
                data: bytes.concat(AddressManager.init.selector)
            })
        );

        ss = SignalService(
            deployProxy({
                name: "signal_service",
                impl: address(new SignalService()),
                data: bytes.concat(SignalService.init.selector),
                registerTo: address(addressManager),
                owner: address(0)
            })
        );

        uint64 gasExcess = 0;
        uint8 quotient = 8;
        uint32 gasTarget = 60_000_000;

        L2 = TaikoL2EIP1559Configurable(
            deployProxy({
                name: "taiko_l2",
                impl: address(new TaikoL2EIP1559Configurable()),
                data: bytes.concat(TaikoL2.init.selector, abi.encode(address(ss), gasExcess))
            })
        );

        L2.setConfigAndExcess(TaikoL2.Config(gasTarget, quotient), gasExcess);

        gasExcess = 195_420_300_100;
        L2skip = SkipBasefeeCheckL2(
            deployProxy({
                name: "taiko_l2",
                impl: address(new SkipBasefeeCheckL2()),
                data: bytes.concat(TaikoL2.init.selector, abi.encode(address(ss), gasExcess))
            })
        );

        L2skip.setConfigAndExcess(TaikoL2.Config(gasTarget, quotient), gasExcess);

        vm.roll(block.number + 1);
        vm.warp(block.timestamp + 30);
    }

    function test_L2_AnchorTx_with_constant_block_time() external {
        for (uint256 i; i < 100; ++i) {
            vm.fee(1);

            vm.prank(L2.GOLDEN_TOUCH_ADDRESS());
            _anchor(BLOCK_GAS_LIMIT);

            vm.roll(block.number + 1);
            vm.warp(block.timestamp + 30);
        }
    }

    function test_L2_AnchorTx_with_decreasing_block_time() external {
        for (uint256 i; i < 32; ++i) {
            vm.fee(1);

            vm.prank(L2.GOLDEN_TOUCH_ADDRESS());
            _anchor(BLOCK_GAS_LIMIT);

            vm.roll(block.number + 1);
            vm.warp(block.timestamp + 30 - i);
        }
    }

    function test_L2_AnchorTx_with_increasing_block_time() external {
        for (uint256 i; i < 30; ++i) {
            vm.fee(1);

            vm.prank(L2.GOLDEN_TOUCH_ADDRESS());
            _anchor(BLOCK_GAS_LIMIT);

            vm.roll(block.number + 1);

            vm.warp(block.timestamp + 30 + i);
        }
    }

    function test_simulation_lower_traffic() external {
        console2.log("LOW TRAFFIC STARTS"); // For parser
        _simulation(100_000, 10_000_000, 1, 8);
        console2.log("LOW TRAFFIC ENDS");
    }

    function test_simulation_higher_traffic() external {
        console2.log("HIGH TRAFFIC STARTS"); // For parser
        _simulation(100_000, 120_000_000, 1, 8);
        console2.log("HIGH TRAFFIC ENDS");
    }

    function test_simulation_target_traffic() external {
        console2.log("TARGET TRAFFIC STARTS"); // For parser
        _simulation(60_000_000, 0, 12, 0);
        console2.log("TARGET TRAFFIC ENDS");
    }

    function _simulation(
        uint256 minGas,
        uint256 maxDiffToMinGas,
        uint8 quickest,
        uint8 maxDiffToQuickest
    )
        internal
    {
        // We need to randomize the:
        // - parent gas used (We should sometimes exceed 150.000.000 gas / 12
        // seconds (to simulate congestion a bit) !!)
        // - the time we fire away an L2 block (anchor transaction).
        // The rest is baked in.
        // initial gas excess issued: 49954623777 (from eip1559_util.py) if we
        // want to stick to the params of 10x Ethereum gas, etc.

        // This variables counts if we reached the 12seconds (L1) height, if so
        // then resets the accumulated parent gas used and increments the L1
        // height number
        uint8 accumulated_seconds = 0;
        uint256 accumulated_parent_gas_per_l1_block = 0;
        uint64 l1Height = uint64(block.number);
        uint64 l1BlockCounter = 0;
        uint64 maxL2BlockCount = 180;
        uint256 allBaseFee = 0;
        uint256 allGasUsed = 0;
        uint256 newRandomWithoutSalt;
        // Simulate 200 L2 blocks
        for (uint256 i; i < maxL2BlockCount; ++i) {
            newRandomWithoutSalt = uint256(
                keccak256(
                    abi.encodePacked(
                        block.prevrandao, msg.sender, block.timestamp, i, newRandomWithoutSalt, salt
                    )
                )
            );

            uint32 currentGasUsed;
            if (maxDiffToMinGas == 0) {
                currentGasUsed = uint32(minGas);
            } else {
                currentGasUsed =
                    uint32(pickRandomNumber(newRandomWithoutSalt, minGas, maxDiffToMinGas));
            }
            salt = uint256(keccak256(abi.encodePacked(currentGasUsed, salt)));
            accumulated_parent_gas_per_l1_block += currentGasUsed;
            allGasUsed += currentGasUsed;

            uint8 currentTimeAhead;
            if (maxDiffToQuickest == 0) {
                currentTimeAhead = uint8(quickest);
            } else {
                currentTimeAhead =
                    uint8(pickRandomNumber(newRandomWithoutSalt, quickest, maxDiffToQuickest));
            }
            accumulated_seconds += currentTimeAhead;

            if (accumulated_seconds >= 12) {
                console2.log(
                    "Gas used per L1 block:", l1Height, ":", accumulated_parent_gas_per_l1_block
                );
                l1Height++;
                l1BlockCounter++;
                accumulated_parent_gas_per_l1_block = 0;
                accumulated_seconds = 0;
            }

            vm.prank(L2.GOLDEN_TOUCH_ADDRESS());
            _anchorSimulation(currentGasUsed, l1Height);
            uint256 currentBaseFee = L2skip.getBasefee(l1Height, currentGasUsed);
            allBaseFee += currentBaseFee;
            console2.log("Actual gas in L2 block is:", currentGasUsed);
            console2.log("L2block to baseFee is:", i, ":", currentBaseFee);
            vm.roll(block.number + 1);

            vm.warp(block.timestamp + currentTimeAhead);
        }

        console2.log("Average wei gas price per L2 block is:", (allBaseFee / maxL2BlockCount));
        console2.log("Average gasUsed per L1 block:", (allGasUsed / l1BlockCounter));
    }

    // calling anchor in the same block more than once should fail
    function test_L2_AnchorTx_revert_in_same_block() external {
        vm.fee(1);

        vm.prank(L2.GOLDEN_TOUCH_ADDRESS());
        _anchor(BLOCK_GAS_LIMIT);

        vm.prank(L2.GOLDEN_TOUCH_ADDRESS());
        vm.expectRevert(); // L2_PUBLIC_INPUT_HASH_MISMATCH
        _anchor(BLOCK_GAS_LIMIT);
    }

    // calling anchor in the same block more than once should fail
    function test_L2_AnchorTx_revert_from_wrong_signer() external {
        vm.fee(1);
        vm.expectRevert();
        _anchor(BLOCK_GAS_LIMIT);
    }

    function test_L2_AnchorTx_signing(bytes32 digest) external {
        (uint8 v, uint256 r, uint256 s) = L2.signAnchor(digest, uint8(1));
        address signer = ecrecover(digest, v + 27, bytes32(r), bytes32(s));
        assertEq(signer, L2.GOLDEN_TOUCH_ADDRESS());

        (v, r, s) = L2.signAnchor(digest, uint8(2));
        signer = ecrecover(digest, v + 27, bytes32(r), bytes32(s));
        assertEq(signer, L2.GOLDEN_TOUCH_ADDRESS());

        vm.expectRevert();
        L2.signAnchor(digest, uint8(0));

        vm.expectRevert();
        L2.signAnchor(digest, uint8(3));
    }

    function _anchor(uint32 parentGasLimit) private {
        bytes32 l1Hash = randBytes32();
        bytes32 l1SignalRoot = randBytes32();
        L2.anchor(l1Hash, l1SignalRoot, 12_345, parentGasLimit);
    }

    function _anchorSimulation(uint32 parentGasLimit, uint64 l1Height) private {
        bytes32 l1Hash = randBytes32();
        bytes32 l1SignalRoot = randBytes32();
        L2skip.anchor(l1Hash, l1SignalRoot, l1Height, parentGasLimit);
    }

    function registerAddress(bytes32 nameHash, address addr) internal {
        addressManager.setAddress(uint64(block.chainid), nameHash, addr);
        console2.log(block.chainid, uint256(nameHash), unicode"→", addr);
    }

    // Semi-random number generator
    function pickRandomNumber(
        uint256 randomNum,
        uint256 lowerLimit,
        uint256 diffBtwLowerAndUpperLimit
    )
        internal
        view
        returns (uint256)
    {
        randomNum = uint256(keccak256(abi.encodePacked(randomNum, salt)));
        return (lowerLimit + (randomNum % diffBtwLowerAndUpperLimit));
    }
}
