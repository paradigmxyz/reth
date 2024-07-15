// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "../TaikoTest.sol";
/*
contract MockVerifier {
    fallback(bytes calldata) external returns (bytes memory) {
        return bytes.concat(keccak256("taiko"));
    }
}
*/
// TODO (dani): remove some code to sub-contracts, this one shall only contain
// shared logics and data.

abstract contract TaikoL1TestBase is TaikoTest {
    AddressManager public addressManager;
    // AssignmentHook public assignmentHook;
    BasedOperator public basedOperator;
    TaikoToken public tko;
    // SignalService public ss;
    TaikoL1 public L1;
    TaikoData.Config conf;
    uint256 internal logCount;
    // PseZkVerifier public pv;
    /* 3 proof verifiers - to fulfill the requirement in BasedOperator.sol */
    MockSgxVerifier public sv1;
    MockSgxVerifier public sv2;
    MockSgxVerifier public sv3;
    VerifierRegistry public vr;
    // SgxAndZkVerifier public sgxZkVerifier;
    // GuardianVerifier public gv;
    // GuardianProver public gp;
    // TaikoA6TierProvider public cp;
    // Bridge public bridge;

    bytes32 public GENESIS_BLOCK_HASH = keccak256("GENESIS_BLOCK_HASH");

    address public L2SS = randAddress();
    address public L2 = randAddress();

    function deployTaikoL1(address addressManager) internal virtual returns (TaikoL1 taikoL1);

    function setUp() public virtual {
        vm.startPrank(Alice);
        vm.roll(20_232_182); //A real Ethereum block number from Jul-04-2024 09:13:47
        vm.warp(1_720_077_269);

        addressManager = AddressManager(
            deployProxy({
                name: "address_manager",
                impl: address(new AddressManager()),
                data: abi.encodeCall(AddressManager.init, (Alice))
            })
        );

        L1 = deployTaikoL1(address(addressManager));
        conf = L1.getConfig();

        console2.log("Address szar:", (address(addressManager)));
        basedOperator = BasedOperator(
            deployProxy({
                name: "operator",
                impl: address(new BasedOperator()),
                data: abi.encodeCall(BasedOperator.init, (Alice, address(addressManager)))
            })
        );

        vr = VerifierRegistry(
            deployProxy({
                name: "verifier_registry",
                impl: address(new VerifierRegistry()),
                data: abi.encodeCall(VerifierRegistry.init, (Alice, address(addressManager)))
            })
        );

        registerAddress("taiko", address(L1));
        registerAddress("operator", address(basedOperator));
        registerAddress("verifier_registry", address(vr));

        //         ss = SignalService(
        //             deployProxy({
        //                 name: "signal_service",
        //                 impl: address(new SignalService()),
        //                 data: bytes.concat(SignalService.init.selector)
        //             })
        //         );

        //         pv = PseZkVerifier(
        //             deployProxy({
        //                 name: "tier_pse_zkevm",
        //                 impl: address(new PseZkVerifier()),
        //                 data: bytes.concat(PseZkVerifier.init.selector,
        // abi.encode(address(addressManager)))
        //             })
        //         );

        address sgxImpl = address(new MockSgxVerifier());
        //Naming is like: 3, 1, 2, is because we need to have incremental order of addresses in
        // BasedOperator, so figured out this is actually the way

        sv1 = MockSgxVerifier(
            deployProxy({
                name: "sgx2", //Name does not matter now, since we check validity via
                    // verifierRegistry
                impl: sgxImpl,
                data: abi.encodeCall(MockSgxVerifier.init, (Alice, address(addressManager)))
            })
        );

        console2.log(address(sv1));

        sv2 = MockSgxVerifier(
            deployProxy({
                name: "sgx3", //Name does not matter now, since we check validity via
                    // verifierRegistry
                impl: sgxImpl,
                data: abi.encodeCall(MockSgxVerifier.init, (Alice, address(addressManager)))
            })
        );

        sv3 = MockSgxVerifier(
            deployProxy({
                name: "sgx1", //Name does not matter now, since we check validity via
                    // verifierRegistry
                impl: sgxImpl,
                data: abi.encodeCall(MockSgxVerifier.init, (Alice, address(addressManager)))
            })
        );

        console2.log(address(sv2));

        // sv2 = SgxVerifier(
        //     deployProxy({
        //         name: "sgx3", //Name does not matter now, since we check validity via
        //             // verifierRegistry
        //         impl: sgxImpl,
        //         data: abi.encodeCall(SgxVerifier.init, (Alice, address(addressManager)))
        //     })
        // );

        console2.log(address(sv3));

        // Bootstrap / add first trusted instance -> SGX code needs some change tho - because
        // changed since taiko-simplified was created first.
        address[] memory initSgxInstances = new address[](1);
        initSgxInstances[0] = SGX_X_0;

        sv1.addInstances(initSgxInstances);
        sv2.addInstances(initSgxInstances);
        sv3.addInstances(initSgxInstances);

        //         address[] memory initSgxInstances = new address[](1);
        //         initSgxInstances[0] = SGX_X_0;
        //         sv.addInstances(initSgxInstances);

        //         sgxZkVerifier = SgxAndZkVerifier(
        //             deployProxy({
        //                 name: "tier_sgx_and_pse_zkevm",
        //                 impl: address(new SgxAndZkVerifier()),
        // data: bytes.concat(SgxAndZkVerifier.init.selector, abi.encode(address(addressManager)))
        //             })
        //         );

        //         gv = GuardianVerifier(
        //             deployProxy({
        //                 name: "guardian_verifier",
        //                 impl: address(new GuardianVerifier()),
        // data: bytes.concat(GuardianVerifier.init.selector, abi.encode(address(addressManager)))
        //             })
        //         );

        //         gp = GuardianProver(
        //             deployProxy({
        //                 name: "guardian_prover",
        //                 impl: address(new GuardianProver()),
        // data: bytes.concat(GuardianProver.init.selector, abi.encode(address(addressManager)))
        //             })
        //         );

        //         setupGuardianProverMultisig();

        //         cp = TaikoA6TierProvider(
        //             deployProxy({
        //                 name: "tier_provider",
        //                 impl: address(new TaikoA6TierProvider()),
        //                 data: bytes.concat(TaikoA6TierProvider.init.selector)
        //             })
        //         );

        //         bridge = Bridge(
        //             payable(
        //                 deployProxy({
        //                     name: "bridge",
        //                     impl: address(new Bridge()),
        //                     data: bytes.concat(Bridge.init.selector, abi.encode(addressManager)),
        //                     registerTo: address(addressManager),
        //                     owner: address(0)
        //                 })
        //             )
        //         );

        //         assignmentHook = AssignmentHook(
        //             deployProxy({
        //                 name: "assignment_hook",
        //                 impl: address(new AssignmentHook()),
        // data: bytes.concat(AssignmentHook.init.selector, abi.encode(address(addressManager)))
        //             })
        //         );

        //         registerAddress("taiko", address(L1));
        //         registerAddress("tier_pse_zkevm", address(pv));
        //         registerAddress("tier_sgx", address(sv));
        //         registerAddress("tier_guardian", address(gv));
        //         registerAddress("tier_sgx_and_pse_zkevm", address(sgxZkVerifier));
        //         registerAddress("tier_provider", address(cp));
        //         registerAddress("signal_service", address(ss));
        //         registerAddress("guardian_prover", address(gp));
        //         registerAddress("bridge", address(bridge));
        //         registerL2Address("taiko", address(L2));
        //         registerL2Address("signal_service", address(L2SS));
        //         registerL2Address("taiko_l2", address(L2));

        //         registerAddress(pv.getVerifierName(300), address(new MockVerifier()));

        tko = TaikoToken(
            deployProxy({
                name: "taiko_token",
                impl: address(new TaikoToken()),
                data: abi.encodeCall(TaikoToken.init, (address(0), address(this))),
                registerTo: address(addressManager)
            })
        );

        L1.init(Alice, address(addressManager), GENESIS_BLOCK_HASH);
        printVariables("init  ");
        vm.stopPrank();

        // Add those 3 to verifier registry
        vm.startPrank(vr.owner());
        vr.addVerifier(address(sv1), "sgx1");
        vr.addVerifier(address(sv2), "sgx2");
        vr.addVerifier(address(sv3), "sgx3");
        vm.stopPrank();
    }

    function proposeBlock(
        address proposer,
        address prover,
        TaikoData.BlockMetadata memory meta,
        bytes4 revertReason
    )
        internal
        returns (TaikoData.BlockMetadata memory)
    {
        // TaikoData.TierFee[] memory tierFees = new TaikoData.TierFee[](5);
        // // Register the tier fees
        // // Based on OPL2ConfigTier we need 3:
        // // - LibTiers.TIER_PSE_ZKEVM;
        // // - LibTiers.TIER_SGX;
        // // - LibTiers.TIER_OPTIMISTIC;
        // // - LibTiers.TIER_GUARDIAN;
        // // - LibTiers.TIER_SGX_AND_PSE_ZKEVM
        // tierFees[0] = TaikoData.TierFee(LibTiers.TIER_OPTIMISTIC, 1 ether);
        // tierFees[1] = TaikoData.TierFee(LibTiers.TIER_SGX, 1 ether);
        // tierFees[2] = TaikoData.TierFee(LibTiers.TIER_PSE_ZKEVM, 2 ether);
        // tierFees[3] = TaikoData.TierFee(LibTiers.TIER_SGX_AND_PSE_ZKEVM, 2 ether);
        // tierFees[4] = TaikoData.TierFee(LibTiers.TIER_GUARDIAN, 0 ether);
        // // For the test not to fail, set the message.value to the highest, the
        // // rest will be returned
        // // anyways
        // uint256 msgValue = 2 ether;

        // AssignmentHook.ProverAssignment memory assignment = AssignmentHook.ProverAssignment({
        //     feeToken: address(0),
        //     tierFees: tierFees,
        //     expiry: uint64(block.timestamp + 60 minutes),
        //     maxBlockId: 0,
        //     maxProposedIn: 0,
        //     metaHash: 0,
        //     signature: new bytes(0)
        // });

        // assignment.signature =
        //     _signAssignment(prover, assignment, address(L1), keccak256(new bytes(txListSize)));

        // (, TaikoData.SlotB memory b) = L1.getStateVariables();

        // uint256 _difficulty;
        // unchecked {
        //     _difficulty = block.prevrandao;
        // }

        // meta.blockHash = blockHash;
        // meta.parentHash = parentHash;

        // meta.timestamp = uint64(block.timestamp);
        // meta.l1Height = uint64(block.number - 1);
        // meta.l1Hash = blockhash(block.number - 1);
        // meta.difficulty = bytes32(_difficulty);
        // meta.gasLimit = gasLimit;

        // TaikoData.HookCall[] memory hookcalls = new TaikoData.HookCall[](1);

        // hookcalls[0] = TaikoData.HookCall(address(assignmentHook), abi.encode(assignment));

        bytes[] memory dummyTxList = new bytes[](1);
        dummyTxList[0] =
            hex"0000000000000000000000000000000000000000000000000000000000000001";
        
        // If blob is used, empty tx list
        bytes[] memory emptyTxList;

        // Input metadata sturct can now support multiple block proposals per L1 TXN
        bytes[] memory metasEncoded = new bytes[](1);
        metasEncoded[0] = abi.encode(meta);

        TaikoData.BlockMetadata[] memory _returnedBlocks = new TaikoData.BlockMetadata[](1);

        if (revertReason == "") {
            vm.prank(proposer, proposer);
            _returnedBlocks = basedOperator.proposeBlock{ value: 1 ether / 10 }(
                metasEncoded, meta.blobUsed == true ? emptyTxList : dummyTxList, prover
            );
        } else {
            vm.prank(proposer, proposer);
            vm.expectRevert(revertReason);
            _returnedBlocks = basedOperator.proposeBlock{ value: 1 ether / 10 }(
                metasEncoded, meta.blobUsed == true ? emptyTxList : dummyTxList, prover
            );
            return meta;
        }

        return _returnedBlocks[0];
    }

    function proveBlock(address prover, bytes memory blockProof) internal {
        vm.prank(prover, prover);
        basedOperator.proveBlock(blockProof);
    }

    function verifyBlock(uint64 count) internal {
        basedOperator.verifyBlocks(count);
    }

    // function setupGuardianProverMultisig() internal {
    //     address[] memory initMultiSig = new address[](5);
    //     initMultiSig[0] = David;
    //     initMultiSig[1] = Emma;
    //     initMultiSig[2] = Frank;
    //     initMultiSig[3] = Grace;
    //     initMultiSig[4] = Henry;

    //     gp.setGuardians(initMultiSig, 3);
    // }

    function registerAddress(bytes32 nameHash, address addr) internal {
        addressManager.setAddress(uint64(block.chainid), nameHash, addr);
        console2.log(block.chainid, uint256(nameHash), unicode"→", addr);
    }

    function registerL2Address(bytes32 nameHash, address addr) internal {
        addressManager.setAddress(conf.chainId, nameHash, addr);
        console2.log(conf.chainId, string(abi.encodePacked(nameHash)), unicode"→", addr);
    }

    // function _signAssignment(
    //     address signer,
    //     AssignmentHook.ProverAssignment memory assignment,
    //     address taikoAddr,
    //     bytes32 blobHash
    // )
    //     internal
    //     view
    //     returns (bytes memory signature)
    // {
    //     uint256 signerPrivateKey;

    //     // In the test suite these are the 3 which acts as provers
    //     if (signer == Alice) {
    //         signerPrivateKey = 0x1;
    //     } else if (signer == Bob) {
    //         signerPrivateKey = 0x2;
    //     } else if (signer == Carol) {
    //         signerPrivateKey = 0x3;
    //     }

    //     (uint8 v, bytes32 r, bytes32 s) = vm.sign(
    //         signerPrivateKey, assignmentHook.hashAssignment(assignment, taikoAddr, blobHash)
    //     );
    //     signature = abi.encodePacked(r, s, v);
    // }

    function createSgxSignatureProof(
        TaikoData.Transition memory tran,
        address newInstance,
        address prover,
        bytes32 metaHash
    )
        internal
        view
        returns (bytes memory signature)
    {
        bytes32 digest = LibPublicInput.hashPublicInputs(
            tran, address(sv1), newInstance, prover, metaHash, L1.getConfig().chainId
        );

        uint256 signerPrivateKey;

        // In the test suite these are the 3 which acts as provers
        if (SGX_X_0 == newInstance) {
            signerPrivateKey = 0x5;
        } else if (SGX_X_1 == newInstance) {
            signerPrivateKey = 0x4;
        }

        (uint8 v, bytes32 r, bytes32 s) = vm.sign(signerPrivateKey, digest);
        signature = abi.encodePacked(r, s, v);
    }

    function giveEthAndTko(address to, uint256 amountTko, uint256 amountEth) internal {
        vm.deal(to, amountEth);
        tko.transfer(to, amountTko);

        vm.prank(to, to);
        tko.approve(address(L1), amountTko);
        // vm.prank(to, to);
        // tko.approve(address(assignmentHook), amountTko);

        console2.log("TKO balance:", to, tko.balanceOf(to));
        console2.log("ETH balance:", to, to.balance);
    }

    function printVariables(string memory comment) internal {
        string memory str = string.concat(
            Strings.toString(logCount++),
            ":[",
            Strings.toString(L1.getLastVerifiedBlockId()),
            unicode"→",
            Strings.toString(L1.getNumOfBlocks()),
            "] // ",
            comment
        );
        console2.log(str);
    }

    function mine(uint256 counts) internal {
        vm.warp(block.timestamp + 12 * counts);
        vm.roll(block.number + counts);
    }

    function createBlockMetaData(
        address coinbase,
        uint64 l2BlockNumber,
        uint32 belowBlockTipHeight, // How many blocks below from current tip (block.id)
        bool blobUsed
    )
        internal
        returns (TaikoData.BlockMetadata memory meta)
    {
        meta.blockHash = randBytes32();

        TaikoData.Block memory parentBlock = L1.getBlock(l2BlockNumber - 1);
        meta.parentMetaHash = parentBlock.metaHash;
        meta.parentBlockHash = parentBlock.blockHash;
        meta.l1Hash = blockhash(block.number - belowBlockTipHeight);
        meta.difficulty = block.prevrandao;
        meta.blobHash = randBytes32();
        meta.coinbase = coinbase;
        meta.l2BlockNumber = l2BlockNumber;
        meta.gasLimit = L1.getConfig().blockMaxGasLimit;
        meta.l1StateBlockNumber = uint32(block.number - belowBlockTipHeight);
        meta.timestamp = uint64(block.timestamp - (belowBlockTipHeight * 12)); // x blocks behind

        if (blobUsed) {
            meta.txListByteOffset = 0;
            meta.txListByteSize = 0;
            meta.blobUsed = true;
        } else {
            meta.txListByteOffset = 0;
            meta.txListByteSize = 32; // Corresponding dummyTxList is set during proposeBlock()
            meta.blobUsed = false;
        }
    }

    function createProofs(
        TaikoData.BlockMetadata memory meta,
        address prover,
        bool threeMockSGXProofs // Used to indicate to "trick" the BasedProver with 3 different (but
            // same code) deployments of SGX verifier - later we can fine tune to have 3 correct,
            // valid proofs.
    )
        internal
        view
        returns (BasedOperator.ProofBatch memory proofBatch)
    {
        // Set metadata
        proofBatch.blockMetadata = meta;

        // Set transition
        TaikoData.Transition memory transition;
        transition.parentBlockHash = L1.getBlock(meta.l2BlockNumber - 1).blockHash;
        transition.blockHash = meta.blockHash;
        proofBatch.transition = transition;

        // Set prover
        proofBatch.prover = prover;

        address newInstance;
        // Keep changing the pub key associated with an instance to avoid
        // attacks,
        // obviously just a mock due to 2 addresses changing all the time.
        (newInstance,) = sv1.instances(0);
        if (newInstance == SGX_X_0) {
            newInstance = SGX_X_1;
        } else {
            newInstance = SGX_X_0;
        }

        BasedOperator.ProofData[] memory proofs = new BasedOperator.ProofData[](3);

        bytes memory signature =
            createSgxSignatureProof(transition, newInstance, prover, keccak256(abi.encode(meta)));

        // The order is on purpose reversed, becase of the L1_INVALID_OR_DUPLICATE_VERIFIER() check
        proofs[0].verifier = sv2;
        proofs[0].proof = bytes.concat(bytes4(0), bytes20(newInstance), signature);

        if (threeMockSGXProofs) {
            // The order is on purpose reversed, becase of the L1_INVALID_OR_DUPLICATE_VERIFIER()
            // check
            proofs[1].verifier = sv1;
            proofs[1].proof = bytes.concat(bytes4(0), bytes20(newInstance), signature);

            proofs[2].verifier = sv3;
            proofs[2].proof = bytes.concat(bytes4(0), bytes20(newInstance), signature);
        } else {
            //Todo(dani): Implement more proof and verifiers when needed/available but for now, not
            // to change the code in BasedOperator, maybe best to mock those 3
        }

        proofBatch.proofs = proofs;
    }
}
