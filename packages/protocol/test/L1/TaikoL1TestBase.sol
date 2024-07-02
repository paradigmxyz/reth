// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "../TaikoTest.sol";
/*
contract MockVerifier {
    fallback(bytes calldata) external returns (bytes memory) {
        return bytes.concat(keccak256("taiko"));
    }
}

// TODO (dani): remove some code to sub-contracts, this one shall only contain
// shared logics and data.
abstract contract TaikoL1TestBase is TaikoTest {
    AddressManager public addressManager;
    AssignmentHook public assignmentHook;
    TaikoToken public tko;
    SignalService public ss;
    TaikoL1 public L1;
    TaikoData.Config conf;
    uint256 internal logCount;
    PseZkVerifier public pv;
    SgxVerifier public sv;
    SgxAndZkVerifier public sgxZkVerifier;
    GuardianVerifier public gv;
    GuardianProver public gp;
    TaikoA6TierProvider public cp;
    Bridge public bridge;

    bytes32 public GENESIS_BLOCK_HASH = keccak256("GENESIS_BLOCK_HASH");

    address public L2SS = randAddress();
    address public L2 = randAddress();

    function deployTaikoL1() internal virtual returns (TaikoL1 taikoL1);

    function setUp() public virtual {
        L1 = deployTaikoL1();
        conf = L1.getConfig();

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
                data: bytes.concat(SignalService.init.selector)
            })
        );

        pv = PseZkVerifier(
            deployProxy({
                name: "tier_pse_zkevm",
                impl: address(new PseZkVerifier()),
                data: bytes.concat(PseZkVerifier.init.selector, abi.encode(address(addressManager)))
            })
        );

        sv = SgxVerifier(
            deployProxy({
                name: "tier_sgx",
                impl: address(new SgxVerifier()),
                data: bytes.concat(SgxVerifier.init.selector, abi.encode(address(addressManager)))
            })
        );

        address[] memory initSgxInstances = new address[](1);
        initSgxInstances[0] = SGX_X_0;
        sv.addInstances(initSgxInstances);

        sgxZkVerifier = SgxAndZkVerifier(
            deployProxy({
                name: "tier_sgx_and_pse_zkevm",
                impl: address(new SgxAndZkVerifier()),
                data: bytes.concat(SgxAndZkVerifier.init.selector, abi.encode(address(addressManager)))
            })
        );

        gv = GuardianVerifier(
            deployProxy({
                name: "guardian_verifier",
                impl: address(new GuardianVerifier()),
                data: bytes.concat(GuardianVerifier.init.selector, abi.encode(address(addressManager)))
            })
        );

        gp = GuardianProver(
            deployProxy({
                name: "guardian_prover",
                impl: address(new GuardianProver()),
                data: bytes.concat(GuardianProver.init.selector, abi.encode(address(addressManager)))
            })
        );

        setupGuardianProverMultisig();

        cp = TaikoA6TierProvider(
            deployProxy({
                name: "tier_provider",
                impl: address(new TaikoA6TierProvider()),
                data: bytes.concat(TaikoA6TierProvider.init.selector)
            })
        );

        bridge = Bridge(
            payable(
                deployProxy({
                    name: "bridge",
                    impl: address(new Bridge()),
                    data: bytes.concat(Bridge.init.selector, abi.encode(addressManager)),
                    registerTo: address(addressManager),
                    owner: address(0)
                })
            )
        );

        assignmentHook = AssignmentHook(
            deployProxy({
                name: "assignment_hook",
                impl: address(new AssignmentHook()),
                data: bytes.concat(AssignmentHook.init.selector, abi.encode(address(addressManager)))
            })
        );

        registerAddress("taiko", address(L1));
        registerAddress("tier_pse_zkevm", address(pv));
        registerAddress("tier_sgx", address(sv));
        registerAddress("tier_guardian", address(gv));
        registerAddress("tier_sgx_and_pse_zkevm", address(sgxZkVerifier));
        registerAddress("tier_provider", address(cp));
        registerAddress("signal_service", address(ss));
        registerAddress("guardian_prover", address(gp));
        registerAddress("bridge", address(bridge));
        registerL2Address("taiko", address(L2));
        registerL2Address("signal_service", address(L2SS));
        registerL2Address("taiko_l2", address(L2));

        registerAddress(pv.getVerifierName(300), address(new MockVerifier()));

        tko = TaikoToken(
            deployProxy({
                name: "taiko_token",
                impl: address(new TaikoToken()),
                data: bytes.concat(
                    TaikoToken.init.selector,
                    abi.encode(
                        "Taiko Token", //
                        "TTKOk",
                        address(this)
                    )
                    ),
                registerTo: address(addressManager),
                owner: address(0)
            })
        );

        L1.init(address(addressManager), GENESIS_BLOCK_HASH);
        printVariables("init  ");
    }

    function proposeBlock(
        address proposer,
        address prover,
        uint32 gasLimit,
        uint24 txListSize
    )
        internal
        returns (
            TaikoData.BlockMetadata memory meta
        )
    {
        TaikoData.TierFee[] memory tierFees = new TaikoData.TierFee[](5);
        // Register the tier fees
        // Based on OPL2ConfigTier we need 3:
        // - LibTiers.TIER_PSE_ZKEVM;
        // - LibTiers.TIER_SGX;
        // - LibTiers.TIER_OPTIMISTIC;
        // - LibTiers.TIER_GUARDIAN;
        // - LibTiers.TIER_SGX_AND_PSE_ZKEVM
        tierFees[0] = TaikoData.TierFee(LibTiers.TIER_OPTIMISTIC, 1 ether);
        tierFees[1] = TaikoData.TierFee(LibTiers.TIER_SGX, 1 ether);
        tierFees[2] = TaikoData.TierFee(LibTiers.TIER_PSE_ZKEVM, 2 ether);
        tierFees[3] = TaikoData.TierFee(LibTiers.TIER_SGX_AND_PSE_ZKEVM, 2 ether);
        tierFees[4] = TaikoData.TierFee(LibTiers.TIER_GUARDIAN, 0 ether);
        // For the test not to fail, set the message.value to the highest, the
        // rest will be returned
        // anyways
        uint256 msgValue = 2 ether;

        AssignmentHook.ProverAssignment memory assignment = AssignmentHook.ProverAssignment({
            feeToken: address(0),
            tierFees: tierFees,
            expiry: uint64(block.timestamp + 60 minutes),
            maxBlockId: 0,
            maxProposedIn: 0,
            metaHash: 0,
            signature: new bytes(0)
        });

        assignment.signature =
            _signAssignment(prover, assignment, address(L1), keccak256(new bytes(txListSize)));

        (, TaikoData.SlotB memory b) = L1.getStateVariables();

        uint256 _difficulty;
        unchecked {
            _difficulty = block.prevrandao * b.numBlocks;
        }

        meta.timestamp = uint64(block.timestamp);
        meta.l1Height = uint64(block.number - 1);
        meta.l1Hash = blockhash(block.number - 1);
        meta.difficulty = bytes32(_difficulty);
        meta.gasLimit = gasLimit;

        TaikoData.HookCall[] memory hookcalls = new TaikoData.HookCall[](1);

        hookcalls[0] = TaikoData.HookCall(address(assignmentHook), abi.encode(assignment));

        vm.prank(proposer, proposer);
        meta = L1.proposeBlock{ value: msgValue }(
            abi.encode(TaikoData.BlockParams(prover, 0, 0, 0, 0, false, 0, hookcalls)),
            new bytes(txListSize)
        );
    }

    function proveBlock(
        address msgSender,
        address prover,
        TaikoData.BlockMetadata memory meta,
        bytes32 parentHash,
        bytes32 blockHash,
        bytes32 signalRoot,
        uint16 tier,
        bytes4 revertReason
    )
        internal
    {
        TaikoData.Transition memory tran = TaikoData.Transition({
            parentHash: parentHash,
            blockHash: blockHash,
            signalRoot: signalRoot,
            graffiti: 0x0
        });

        bytes32 instance =
            pv.calcInstance(tran, prover, keccak256(abi.encode(meta)), meta.blobHash, 0);

        TaikoData.TierProof memory proof;
        proof.tier = tier;
        {
            PseZkVerifier.ZkEvmProof memory zkProof;
            zkProof.verifierId = 300;
            zkProof.zkp = bytes.concat(
                bytes16(0),
                bytes16(instance),
                bytes16(0),
                bytes16(uint128(uint256(instance))),
                new bytes(100)
            );

            proof.data = abi.encode(zkProof);
        }

        address newInstance;
        // Keep changing the pub key associated with an instance to avoid
        // attacks,
        // obviously just a mock due to 2 addresses changing all the time.
        (newInstance,) = sv.instances(0);
        if (newInstance == SGX_X_0) {
            newInstance = SGX_X_1;
        } else {
            newInstance = SGX_X_0;
        }

        if (tier == LibTiers.TIER_SGX) {
            bytes memory signature =
                createSgxSignatureProof(tran, newInstance, prover, keccak256(abi.encode(meta)));

            proof.data = bytes.concat(bytes4(0), bytes20(newInstance), signature);
        }

        if (tier == LibTiers.TIER_SGX_AND_PSE_ZKEVM) {
            bytes memory signature =
                createSgxSignatureProof(tran, newInstance, prover, keccak256(abi.encode(meta)));

            bytes memory sgxProof = bytes.concat(bytes4(0), bytes20(newInstance), signature);
            // Concatenate SGX and ZK (in this order)
            proof.data = bytes.concat(sgxProof, proof.data);
        }

        if (tier == LibTiers.TIER_GUARDIAN) {
            proof.data = "";

            // Grant 2 signatures, 3rd might be a revert
            vm.prank(David, David);
            gp.approve(meta, tran, proof);
            vm.prank(Emma, Emma);
            gp.approve(meta, tran, proof);

            if (revertReason != "") {
                vm.prank(Frank, Frank);
                vm.expectRevert(); // Revert reason is 'wrapped' so will not be
                    // identical to the expectedRevert
                gp.approve(meta, tran, proof);
            } else {
                vm.prank(Frank, Frank);
                gp.approve(meta, tran, proof);
            }
        } else {
            if (revertReason != "") {
                vm.prank(msgSender, msgSender);
                vm.expectRevert(revertReason);
                L1.proveBlock(meta.id, abi.encode(meta, tran, proof));
            } else {
                vm.prank(msgSender, msgSender);
                L1.proveBlock(meta.id, abi.encode(meta, tran, proof));
            }
        }
    }

    function verifyBlock(address, uint64 count) internal {
        L1.verifyBlocks(count);
    }

    function setupGuardianProverMultisig() internal {
        address[] memory initMultiSig = new address[](5);
        initMultiSig[0] = David;
        initMultiSig[1] = Emma;
        initMultiSig[2] = Frank;
        initMultiSig[3] = Grace;
        initMultiSig[4] = Henry;

        gp.setGuardians(initMultiSig, 3);
    }

    function registerAddress(bytes32 nameHash, address addr) internal {
        addressManager.setAddress(uint64(block.chainid), nameHash, addr);
        console2.log(block.chainid, uint256(nameHash), unicode"→", addr);
    }

    function registerL2Address(bytes32 nameHash, address addr) internal {
        addressManager.setAddress(conf.chainId, nameHash, addr);
        console2.log(conf.chainId, string(abi.encodePacked(nameHash)), unicode"→", addr);
    }

    function _signAssignment(
        address signer,
        AssignmentHook.ProverAssignment memory assignment,
        address taikoAddr,
        bytes32 blobHash
    )
        internal
        view
        returns (bytes memory signature)
    {
        uint256 signerPrivateKey;

        // In the test suite these are the 3 which acts as provers
        if (signer == Alice) {
            signerPrivateKey = 0x1;
        } else if (signer == Bob) {
            signerPrivateKey = 0x2;
        } else if (signer == Carol) {
            signerPrivateKey = 0x3;
        }

        (uint8 v, bytes32 r, bytes32 s) = vm.sign(
            signerPrivateKey, assignmentHook.hashAssignment(assignment, taikoAddr, blobHash)
        );
        signature = abi.encodePacked(r, s, v);
    }

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
        bytes32 digest = sv.getSignedHash(tran, newInstance, prover, metaHash);

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
        vm.prank(to, to);
        tko.approve(address(assignmentHook), amountTko);

        console2.log("TKO balance:", to, tko.balanceOf(to));
        console2.log("ETH balance:", to, to.balance);
    }

    function printVariables(string memory comment) internal {
        (TaikoData.SlotA memory a, TaikoData.SlotB memory b) = L1.getStateVariables();

        string memory str = string.concat(
            Strings.toString(logCount++),
            ":[",
            Strings.toString(b.lastVerifiedBlockId),
            unicode"→",
            Strings.toString(b.numBlocks),
            "]"
        );

        str = string.concat(
            str,
            " nextEthDepositToProcess:",
            Strings.toString(a.nextEthDepositToProcess),
            " numEthDeposits:",
            Strings.toString(a.numEthDeposits),
            " // ",
            comment
        );
        console2.log(str);
    }

    function mine(uint256 counts) internal {
        vm.warp(block.timestamp + 20 * counts);
        vm.roll(block.number + counts);
    }
}
*/