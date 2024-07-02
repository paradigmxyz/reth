// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "../TaikoTest.sol";

contract TestSignalService is TaikoTest {
    AddressManager addressManager;
    SignalService signalService;
    SignalService destSignalService;
    DummyCrossChainSync crossChainSync;
    uint64 public destChainId = 7;

    function setUp() public {
        vm.startPrank(Alice);
        vm.deal(Alice, 1 ether);
        vm.deal(Bob, 1 ether);

        addressManager = AddressManager(
            deployProxy({
                name: "address_manager",
                impl: address(new AddressManager()),
                data: bytes.concat(AddressManager.init.selector),
                registerTo: address(addressManager),
                owner: address(0)
            })
        );

        signalService = SignalService(
            deployProxy({
                name: "signal_service",
                impl: address(new SignalService()),
                data: bytes.concat(SignalService.init.selector)
            })
        );

        destSignalService = SignalService(
            deployProxy({
                name: "signal_service",
                impl: address(new SignalService()),
                data: bytes.concat(SignalService.init.selector)
            })
        );

        crossChainSync = DummyCrossChainSync(
            deployProxy({
                name: "dummy_cross_chain_sync",
                impl: address(new DummyCrossChainSync()),
                data: ""
            })
        );

        register(address(addressManager), "signal_service", address(destSignalService), destChainId);

        register(address(addressManager), "taiko", address(crossChainSync), destChainId);

        vm.stopPrank();
    }

    function test_SignalService_sendSignal_revert() public {
        vm.expectRevert(SignalService.SS_INVALID_SIGNAL.selector);
        signalService.sendSignal(0);
    }

    function test_SignalService_isSignalSent_revert() public {
        bytes32 signal = bytes32(uint256(1));
        vm.expectRevert(SignalService.SS_INVALID_APP.selector);
        signalService.isSignalSent(address(0), signal);

        signal = bytes32(uint256(0));
        vm.expectRevert(SignalService.SS_INVALID_SIGNAL.selector);
        signalService.isSignalSent(Alice, signal);
    }

    function test_SignalService_sendSignal_isSignalSent() public {
        vm.startPrank(Alice);
        bytes32 signal = bytes32(uint256(1));
        signalService.sendSignal(signal);

        assertTrue(signalService.isSignalSent(Alice, signal));
    }

    function test_SignalService_getSignalSlot() public {
        vm.startPrank(Alice);
        for (uint8 i = 1; i < 100; ++i) {
            bytes32 signal = bytes32(block.prevrandao + i);
            signalService.sendSignal(signal);

            assertTrue(signalService.isSignalSent(Alice, signal));
        }
    }

    function test_SignalService_proveSignalReceived_L1_L2() public {
        uint64 chainId = 11_155_111; // Created the proofs on a deployed Sepolia
            // contract, this is why this chainId.
        address app = 0x927a146e18294efb36edCacC99D9aCEA6aB16b95; // Mock app,
            // actually it is an EOA, but it is ok for tests!
        bytes32 signal = 0x21761f7cd1af3972774272b39a0f4602dbcd418325cddb14e156b4bb073d52a8;
        bytes memory inclusionProof =
            hex"e5a4e3a1209749684f52b5c0717a7ca78127fb56043d637d81763c04e9d30ba4d4746d56e901"; //eth_getProof's
            // result RLP encoded storage proof
        bytes32 signalRoot = 0xf7916f389ccda56e3831e115238b7389b30750886785a3c21265601572698f0f; //eth_getProof
            // result's storage hash

        vm.startPrank(Alice);
        signalService.authorize(address(crossChainSync), bytes32(uint256(block.chainid)));

        crossChainSync.setSyncedData("", signalRoot);

        SignalService.Proof memory p;
        SignalService.Hop[] memory h;
        p.crossChainSync = address(crossChainSync);
        p.height = 10;
        p.storageProof = inclusionProof;
        p.hops = h;

        bool isSignalReceived =
            signalService.proveSignalReceived(chainId, app, signal, abi.encode(p));
        assertEq(isSignalReceived, true);
    }

    function test_SignalService_proveSignalReceived_L2_L2() public {
        uint64 chainId = 11_155_111; // Created the proofs on a deployed
            // Sepolia contract, this is why this chainId. This works as a
            // static 'chainId' becuase i imitated 2 contracts (L2A and L1
            // Signal Service contracts) on Sepolia.
        address app = 0x927a146e18294efb36edCacC99D9aCEA6aB16b95; // Mock app,
            // actually it is an EOA, but it is ok for tests! Same applies here,
            // i imitated everything with one 'app' (Bridge) with my same EOA
            // wallet.
        bytes32 signal_of_L2A_msgHash =
            0x21761f7cd1af3972774272b39a0f4602dbcd418325cddb14e156b4bb073d52a8;
        bytes memory inclusionProof_of_L2A_msgHash =
            hex"e5a4e3a1209749684f52b5c0717a7ca78127fb56043d637d81763c04e9d30ba4d4746d56e901"; //eth_getProof's
            // result RLP encoded storage proof
        bytes32 signalRoot_of_L2 =
            0xf7916f389ccda56e3831e115238b7389b30750886785a3c21265601572698f0f; //eth_getProof
            // result's storage hash
        bytes memory hop_inclusionProof_from_L1_SignalService =
            hex"e5a4e3a120bade38703a7b19341b10a4dd482698dc8ffdd861e83ce41de2980bed39b6a02501"; //eth_getProof's
            // result RLP encoded storage proof
        bytes32 l1_common_signalService_root =
            0x5c5fd43df8bcd7ad44cfcae86ed73a11e0baa9a751f0b520d029358ea284833b; //eth_getProof
            // result's storage hash

        // Important to note, we need to have authorized the "relayers'
        // addresses" on the source chain we are claiming.
        // (TaikoL1 or TaikoL2 depending on where we are)
        vm.startPrank(Alice);
        signalService.authorize(address(crossChainSync), bytes32(block.chainid));
        signalService.authorize(address(app), bytes32(uint256(chainId)));

        vm.startPrank(Alice);
        addressManager.setAddress(chainId, "taiko", app);

        crossChainSync.setSyncedData("", l1_common_signalService_root);

        SignalService.Proof memory p;
        p.crossChainSync = address(crossChainSync);
        p.height = 10;
        p.storageProof = inclusionProof_of_L2A_msgHash;

        // Imagine this scenario: L2A to L2B bridging.
        // The 'hop' proof is the one that proves to L2B, that L1 Signal service
        // contains the signalRoot (as storage slot / leaf) with value 0x1.
        // The 'normal' proof is the one which proves that the resolving
        // hop.signalRoot is the one which belongs to L2A, and the proof is
        // accordingly.
        SignalService.Hop[] memory h = new SignalService.Hop[](1);
        h[0].signalRootRelay = app;
        h[0].signalRoot = signalRoot_of_L2;
        h[0].storageProof = hop_inclusionProof_from_L1_SignalService;

        p.hops = h;

        bool isSignalReceived =
            signalService.proveSignalReceived(chainId, app, signal_of_L2A_msgHash, abi.encode(p));
        assertEq(isSignalReceived, true);
    }
}
