// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import "../TaikoTest.sol";
import "forge-std/console2.sol";

contract MockSignalService is SignalService {
    function _verifyHopProof(
        uint64, /*chainId*/
        address, /*app*/
        bytes32, /*signal*/
        bytes32, /*value*/
        HopProof memory, /*hop*/
        address /*relay*/
    )
        internal
        pure
        override
        returns (bytes32)
    {
        // Skip verifying the merkle proof entirely
        return bytes32(uint256(789));
    }
}

contract TestSignalService is TaikoTest {
    AddressManager addressManager;
    MockSignalService signalService;
    SignalService realSignalService;
    uint64 public destChainId = 7;
    address taiko;

    function setUp() public {
        vm.startPrank(Alice);
        vm.deal(Alice, 1 ether);
        vm.deal(Bob, 1 ether);

        addressManager = AddressManager(
            deployProxy({
                name: "address_manager",
                impl: address(new AddressManager()),
                data: abi.encodeCall(AddressManager.init, (address(0))),
                registerTo: address(addressManager)
            })
        );

        signalService = MockSignalService(
            deployProxy({
                name: "signal_service",
                impl: address(new MockSignalService()),
                data: abi.encodeCall(SignalService.init, (address(0), address(addressManager)))
            })
        );

        realSignalService = SignalService(
            deployProxy({
                name: "signal_service",
                impl: address(new SignalService()),
                data: abi.encodeCall(SignalService.init, (Alice, address(addressManager)))
            })
        );

        taiko = randAddress();
        signalService.authorize(taiko, true);
        realSignalService.authorize(Alice, true);
        vm.stopPrank();
    }

    function test_real_signal() public {
        vm.chainId(167_001);

        bytes memory proof =
            hex"0000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000028c5900000000000000000000000000000000000000000000000000000000000015c27a889e6436fc1cde7827f75217adf5371afb14cc56860e6d9032ba5e28214819000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000c000000000000000000000000000000000000000000000000000000000000005000000000000000000000000000000000000000000000000000000000000000003000000000000000000000000000000000000000000000000000000000000006000000000000000000000000000000000000000000000000000000000000002a000000000000000000000000000000000000000000000000000000000000003800000000000000000000000000000000000000000000000000000000000000214f90211a0dfa513509c34b62c66488e60634c6fd42fe00397e4e1d15d5e70f227ded60befa05bdb02a2eca8f311b9671483176ec61fae7edcaf62b04a28c44cc2192e9d0f46a01cc45e358bfc5242aaf32f63dee58b9aa3f4113be91b2b50bb5740beda4fde25a0d92467760a1a9357492b426c682426d81af5cb713839647e49b13e01b02d6440a0c84062f286e3581246bccf7d0b7f82366e880161f91879ebef0180c9c93c941aa0db20b735c7f1053aceb0828f861d0b7e33febd12a123bc0352c929718b845faaa0247a3b41495f2b92708771026d7099e824051e275b8a29e82876030437b67c0aa0477ffe5998d9bc8b5866d020bb1d184b62cd6ab90475bc5cf9ec0a55c417c28ba074ecd264e5eb045d4d93d6670df1364257028d7b7fdda9c4eb627d7cd744780ba02221fdd890129df643dc88a6767c7d586fac3fd573ec069b15232f7b26a7ce28a06ea5ac730ebf7a40efe4cf416fac7ad0bdd3afcfb58d6df8c5915050822a3359a03ec8023d3660e15a8ba0ab69a1ed8ae5f121d3221587d14b1ee11b92fef2f92ca03ed73d6c820ff05ed2b08021ffa9a9cf754e376e26f80ba5fe8ba2906d398f8fa0e8e7f865b0d0ece00599f085d5b3e1ba7ca723b8918242001fe1939e1c5e4636a023f41a3b7932420372da53ae441f422ca8c5da9706a79ff47a81c5c8c1fb4917a003a143ebcd7f0dc5385606c701eb2b8245300e1ea72dd478ebf3dd894b173b598000000000000000000000000000000000000000000000000000000000000000000000000000000000000000b3f8b180a0887fcd34179304e7cd2b4e20e2b02a4a8f86479c938ef9f250afa70389b005f9808080a044716b385f8d9459d36317d101408b5ac6918cf2ca6fec073f6bc6a24a3a04e4a024c54ee716f3386163e13b31e8987471a988be429e3aef879217e22b7760561ca00140f6012398f45a3130df7f78f20f7c40bf0d1074b88d3cf981bf0afae32e2580808080a03b0365c6777cd412b8762e2e8216a2fab245f1c83111e08f09c20ae4ed8628e88080808000000000000000000000000000000000000000000000000000000000000000000000000000000000000000006bf869a02016202fe7055f067ad86805f2e5a7f909257e295658bcbfc2b2cb8c3127fb9db846f8440180a014f07e11fa9eac150c017a5fea958a3b73935da8b057843d3314bc53acbd00e5a0dc679fd48cf611aa38e906adc93928c5f8f6fae534978ea2fa4f5935f5ed1b2c000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000500000000000000000000000000000000000000000000000000000000000000a000000000000000000000000000000000000000000000000000000000000002e0000000000000000000000000000000000000000000000000000000000000052000000000000000000000000000000000000000000000000000000000000006e000000000000000000000000000000000000000000000000000000000000007600000000000000000000000000000000000000000000000000000000000000214f90211a09e1b6a91ab7ab54dc82ca4bf7c621663b37d09fd0adfc6ee5b04d61d1674be28a0be9221596e9d2200855bf3eed5237bf998f95a94d46f9045c3d15138262aa336a08a468dcca4b10bc41cbfbe1cff581839da9efb3a6453a1a9d51c623393056b75a0eb149f5f12c6e13cf0e0bdff4a87870c1d2a15f512d8610d938ff91420e567bca0ae4668eccb3ec464e47550870fb978fe7840ea30cc94e9b4983ea213de536caca0fd30849f4ce21a0bf9e93e7ff6681ba9f8c4c23d9c2b32aaabbba735cb3ad8bca03a61ccbbd269b701d9da7568421d47355b9b07f88cb1a8b559bcbbda60cdc588a04ccb3f9257808d764c277e5825d005516ac955f64e59d0e7ab2f94b1a4fc4c17a0697d43c2b13982e7971e4bd05cca3a3714163333c764d5383d1f5e642f6b9038a0fad2df5c5417b57cf90bac46a87f3dbb68a02fc415d382b7880ca203998e5848a0456c9736422257556e259b1ec6ef1f57603db9140a81a0537d3efa9392fa1396a0d75f6fb980e2a441be4e5da97f59b411caef24b0ddbbdf69eb51e8294c7721d0a030358f3b1834ef739810ca31f4fabd79f43dc1af8f8ece3b66cbdfec1e2f91d7a0191280d4afd9c5d9e493b78a155abbb8e5bb61754672c3deeb3002c337c7376ea0deb7ba981af9635c6b0df1de8dd515128592e2fb80bb760279f8492d8d4caa8ba070096993175dea6432f4243ae88ccdfacd67453e4d018bfe2f43dee3b5d831ad800000000000000000000000000000000000000000000000000000000000000000000000000000000000000214f90211a0994000eab355e9641a1b22339676cca81f018f367ec829b07ff569bd3f418f43a09a373f01670a460ef4d7f4e2893d5fd9411b696b4a943f18e988912e8bab8349a087849ade71c21e99bd2cff5dbb222ef04fd42d381d488936de43b7ef4380b4d3a072278a26bebb48de2be3ed56fda4bb05cfc430384f76a46402880b16fc56b823a02051ad643d886b6aebaca99b95efbdfb299ff1dbd8f7cba92431a6c83da68381a0fc9d08a35f7850ef9e6932f0b2ccca5e77dee26da198e942935436e7a04f9ba2a01cd8eb71fa95b520e37763506000751e573cdb4d6b7f22809fc7725569bf5362a05097491bc4a3dc25f339fa3be312044e0fb85445bd8bbfba76de7cc8de278db9a0057aca47ca23862aa56aa61e58822f9010782645309b24d7ed41fc564342bf97a0c0bc073a3474c0bd105c3db6a915598559501abe06e9ec9cf9f7345c440ade8ca054be9f6d8226aba0c1b4c530000991fd8476e0c99faecc3f3703751d4929d994a01d192e93484bca5e70661e1655cdc58994e467e58dc6fa6349e3339d9263f8e3a0de752f43804851bc139350b4d09d8eeae52d3793e5a77d966bdcc543e8ed4a07a08d17a09e17828697ba29022f714d57588b55942120b44adb3e7f4311b11a60dca05989405f26fd35e72e532c6528228e90fc3a010eb0d87feca60557fa18b45896a0dcf2645898dead212b4330054c56e51f10af5d3f3bfc64a8f73cdf1bd6617e0d800000000000000000000000000000000000000000000000000000000000000000000000000000000000000194f90191a00415edc1050fe50ad35dbcf7960f64d127b236a1c0d7653a21d75f3cacb529c980a04c18411340e7a2669fd694dee4413c834cf59b13b54107b832503769f2c942b2a0c0910bd1e2e47c03541323952c9da9da3e5807c306b2ca0b4af696508b38f82a80a034e02eebc9d6a274c89c7789fd03eb12333e65346d9453cca75e4a02d7b2992ea0ff68d74f45306203073844aee925a4cc4ae4d78cc032f54504ee9c9335cd32dd8080a09c1849766089cd1349cef938726d7a9e7dc9598fb261d136d9a58e0b7caf275ba0f19ae8192e9b0aadf26d08fbe4f32d0f8949afc2e172c90c834685302ea69bbca0e10bf29e7ae1512acb04c2c1d63c124c4d02e191374f93fefce052420ce55e15a082010d47803c23ef3a2be7b3b089572c2a59de5a9173e9f9404bb71d367e0adaa0a29475985f39e34dec60f13ac4ca2370041873aa30f823b0d0d301998136804ba0d7acb7203bbbd9c87cd646416284ef32bbae269bf178e56127f4a69d6f7a6e43a0224da002242f29898a82f586a50143afa77334954d4581e61266a245c13254c7800000000000000000000000000000000000000000000000000000000000000000000000000000000000000053f851808080a024635e394cfa76468c00494a6e4fc0a50dd27d737c9e41861d32a8be31e3a38d808080808080a06f7c75a0076a5802c84d3d370baefd6b6655641f648e087b60a86c402b07ba84808080808080000000000000000000000000000000000000000000000000000000000000000000000000000000000000000044f8429f20b4780cdd5e2149e06b1a3cd443645775c177c33344f9f36e535023c39e1fa1a02299879cc2fe4c05d8b15238cbf4b15f35a7d434084d279d42f462c90f02b54400000000000000000000000000000000000000000000000000000000";
        bytes32 msgHash = hex"2299879cc2fe4c05d8b15238cbf4b15f35a7d434084d279d42f462c90f02b544";

        address srcSignalService = 0x604C61d6618AaCdF7a7A2Fe4c42E35Ecba32AE75;
        address srcBridge = 0xde5B0e8a8034eF30a8b71d78e658C85dFE3FC657;

        vm.prank(Alice);
        addressManager.setAddress(32_382, "signal_service", srcSignalService);

        bytes32 stateRoot = hex"7a889e6436fc1cde7827f75217adf5371afb14cc56860e6d9032ba5e28214819";
        uint64 blockId = 5570;
        vm.prank(Alice);
        realSignalService.syncChainData(32_382, LibStrings.H_STATE_ROOT, blockId, stateRoot);

        realSignalService.proveSignalReceived(32_382, srcBridge, msgHash, proof);
    }

    function test_SignalService_sendSignal_revert() public {
        vm.expectRevert(EssentialContract.ZERO_VALUE.selector);
        signalService.sendSignal(0);
    }

    function test_SignalService_isSignalSent_revert() public {
        bytes32 signal = bytes32(uint256(1));
        vm.expectRevert(EssentialContract.ZERO_ADDRESS.selector);
        signalService.isSignalSent(address(0), signal);

        signal = bytes32(uint256(0));
        vm.expectRevert(EssentialContract.ZERO_VALUE.selector);
        signalService.isSignalSent(Alice, signal);
    }

    function test_SignalService_sendSignal_isSignalSent() public {
        vm.startPrank(Alice);
        bytes32 signal = bytes32(uint256(1));
        signalService.sendSignal(signal);

        assertTrue(signalService.isSignalSent(Alice, signal));
    }

    function test_SignalService_proveSignalReceived_revert_invalid_chainid_or_signal() public {
        SignalService.HopProof[] memory proofs = new SignalService.HopProof[](1);

        // app being address(0) will revert
        vm.expectRevert(EssentialContract.ZERO_ADDRESS.selector);
        signalService.proveSignalReceived({
            _chainId: 1,
            _app: address(0),
            _signal: randBytes32(),
            _proof: abi.encode(proofs)
        });

        // signal being 0 will revert
        vm.expectRevert(EssentialContract.ZERO_VALUE.selector);
        signalService.proveSignalReceived({
            _chainId: uint64(block.chainid),
            _app: randAddress(),
            _signal: 0,
            _proof: abi.encode(proofs)
        });
    }

    function test_SignalService_proveSignalReceived_revert_malformat_proof() public {
        // "undecodable proof" is not decodeable into SignalService.HopProof[] memory
        vm.expectRevert();
        signalService.proveSignalReceived({
            _chainId: 0,
            _app: randAddress(),
            _signal: randBytes32(),
            _proof: "undecodable proof"
        });
    }

    function test_SignalService_proveSignalReceived_revert_src_signal_service_not_registered()
        public
    {
        uint64 srcChainId = uint64(block.chainid - 1);

        // Did not call the following, so revert with RESOLVER_ZERO_ADDR
        //   vm.prank(Alice);
        //   addressManager.setAddress(srcChainId, "signal_service", randAddress());

        SignalService.HopProof[] memory proofs = new SignalService.HopProof[](1);

        vm.expectRevert(
            abi.encodeWithSelector(
                AddressResolver.RESOLVER_ZERO_ADDR.selector,
                srcChainId,
                strToBytes32("signal_service")
            )
        );
        signalService.proveSignalReceived({
            _chainId: srcChainId,
            _app: randAddress(),
            _signal: randBytes32(),
            _proof: abi.encode(proofs)
        });
    }

    function test_SignalService_proveSignalReceived_revert_zero_size_proof() public {
        uint64 srcChainId = uint64(block.chainid - 1);

        vm.prank(Alice);
        addressManager.setAddress(srcChainId, "signal_service", randAddress());

        // proofs.length must > 0 in order not to revert
        SignalService.HopProof[] memory proofs = new SignalService.HopProof[](0);

        vm.expectRevert(SignalService.SS_EMPTY_PROOF.selector);
        signalService.proveSignalReceived({
            _chainId: srcChainId,
            _app: randAddress(),
            _signal: randBytes32(),
            _proof: abi.encode(proofs)
        });
    }

    function test_SignalService_proveSignalReceived_revert_last_hop_incorrect_chainid() public {
        uint64 srcChainId = uint64(block.chainid - 1);

        vm.prank(Alice);
        addressManager.setAddress(srcChainId, "signal_service", randAddress());

        SignalService.HopProof[] memory proofs = new SignalService.HopProof[](1);

        // proofs[0].chainId must be block.chainid in order not to revert
        proofs[0].chainId = uint64(block.chainid + 1);
        proofs[0].blockId = 1;

        vm.expectRevert(SignalService.SS_INVALID_LAST_HOP_CHAINID.selector);
        signalService.proveSignalReceived({
            _chainId: srcChainId,
            _app: randAddress(),
            _signal: randBytes32(),
            _proof: abi.encode(proofs)
        });
    }

    function test_SignalService_proveSignalReceived_revert_mid_hop_incorrect_chainid() public {
        uint64 srcChainId = uint64(block.chainid - 1);

        vm.prank(Alice);
        addressManager.setAddress(srcChainId, "signal_service", randAddress());

        SignalService.HopProof[] memory proofs = new SignalService.HopProof[](2);

        // proofs[0].chainId must NOT be block.chainid in order not to revert
        proofs[0].chainId = uint64(block.chainid);
        proofs[0].blockId = 1;

        vm.expectRevert(SignalService.SS_INVALID_MID_HOP_CHAINID.selector);
        signalService.proveSignalReceived({
            _chainId: srcChainId,
            _app: randAddress(),
            _signal: randBytes32(),
            _proof: abi.encode(proofs)
        });
    }

    function test_SignalService_proveSignalReceived_revert_mid_hop_not_registered() public {
        uint64 srcChainId = uint64(block.chainid + 1);

        vm.prank(Alice);
        addressManager.setAddress(srcChainId, "signal_service", randAddress());

        SignalService.HopProof[] memory proofs = new SignalService.HopProof[](2);

        // proofs[0].chainId must NOT be block.chainid in order not to revert
        proofs[0].chainId = srcChainId + 1;
        proofs[0].blockId = 1;

        vm.expectRevert(
            abi.encodeWithSelector(
                AddressResolver.RESOLVER_ZERO_ADDR.selector,
                proofs[0].chainId,
                strToBytes32("signal_service")
            )
        );

        signalService.proveSignalReceived({
            _chainId: srcChainId,
            _app: randAddress(),
            _signal: randBytes32(),
            _proof: abi.encode(proofs)
        });
    }

    function test_SignalService_proveSignalReceived_local_chaindata_not_found() public {
        uint64 srcChainId = uint64(block.chainid + 1);

        vm.prank(Alice);
        addressManager.setAddress(srcChainId, "signal_service", randAddress());

        SignalService.HopProof[] memory proofs = new SignalService.HopProof[](1);

        proofs[0].chainId = uint64(block.chainid);
        proofs[0].blockId = 1;

        // the proof is a storage proof
        proofs[0].accountProof = new bytes[](0);
        proofs[0].storageProof = new bytes[](10);

        vm.expectRevert(SignalService.SS_SIGNAL_NOT_FOUND.selector);
        signalService.proveSignalReceived({
            _chainId: srcChainId,
            _app: randAddress(),
            _signal: randBytes32(),
            _proof: abi.encode(proofs)
        });

        // the proof is a full proof
        proofs[0].accountProof = new bytes[](1);

        vm.expectRevert(SignalService.SS_SIGNAL_NOT_FOUND.selector);
        signalService.proveSignalReceived({
            _chainId: srcChainId,
            _app: randAddress(),
            _signal: randBytes32(),
            _proof: abi.encode(proofs)
        });
    }

    function test_SignalService_proveSignalReceived_one_hop_cache_signal_root() public {
        uint64 srcChainId = uint64(block.chainid + 1);

        vm.prank(Alice);
        addressManager.setAddress(srcChainId, "signal_service", randAddress());

        SignalService.HopProof[] memory proofs = new SignalService.HopProof[](1);

        proofs[0].chainId = uint64(block.chainid);
        proofs[0].blockId = 1;
        proofs[0].rootHash = randBytes32();

        // the proof is a storage proof
        proofs[0].accountProof = new bytes[](0);
        proofs[0].storageProof = new bytes[](10);

        vm.expectRevert(SignalService.SS_SIGNAL_NOT_FOUND.selector);
        signalService.proveSignalReceived({
            _chainId: srcChainId,
            _app: randAddress(),
            _signal: randBytes32(),
            _proof: abi.encode(proofs)
        });

        // relay the signal root
        vm.prank(taiko);
        signalService.syncChainData(
            srcChainId, LibStrings.H_SIGNAL_ROOT, proofs[0].blockId, proofs[0].rootHash
        );
        signalService.proveSignalReceived({
            _chainId: srcChainId,
            _app: randAddress(),
            _signal: randBytes32(),
            _proof: abi.encode(proofs)
        });

        vm.prank(Alice);
        signalService.authorize(taiko, false);

        vm.expectRevert(SignalService.SS_UNAUTHORIZED.selector);
        vm.prank(taiko);
        signalService.syncChainData(
            srcChainId, LibStrings.H_SIGNAL_ROOT, proofs[0].blockId, proofs[0].rootHash
        );
    }

    function test_SignalService_proveSignalReceived_one_hop_state_root() public {
        uint64 srcChainId = uint64(block.chainid + 1);

        vm.prank(Alice);
        addressManager.setAddress(srcChainId, "signal_service", randAddress());

        SignalService.HopProof[] memory proofs = new SignalService.HopProof[](1);

        proofs[0].chainId = uint64(block.chainid);
        proofs[0].blockId = 1;
        proofs[0].rootHash = randBytes32();

        // the proof is a full merkle proof
        proofs[0].accountProof = new bytes[](1);
        proofs[0].storageProof = new bytes[](10);

        vm.expectRevert(SignalService.SS_SIGNAL_NOT_FOUND.selector);
        signalService.proveSignalReceived({
            _chainId: srcChainId,
            _app: randAddress(),
            _signal: randBytes32(),
            _proof: abi.encode(proofs)
        });

        // relay the state root
        vm.prank(taiko);
        signalService.syncChainData(
            srcChainId, LibStrings.H_STATE_ROOT, proofs[0].blockId, proofs[0].rootHash
        );

        // Should not revert
        signalService.proveSignalReceived({
            _chainId: srcChainId,
            _app: randAddress(),
            _signal: randBytes32(),
            _proof: abi.encode(proofs)
        });

        assertEq(
            signalService.isChainDataSynced(
                srcChainId, LibStrings.H_SIGNAL_ROOT, proofs[0].blockId, bytes32(uint256(789))
            ),
            false
        );
    }

    function test_SignalService_proveSignalReceived_multiple_hops_no_caching() public {
        uint64 srcChainId = uint64(block.chainid + 1);

        vm.prank(Alice);
        addressManager.setAddress(srcChainId, "signal_service", randAddress());

        SignalService.HopProof[] memory proofs = new SignalService.HopProof[](3);

        // first hop with full merkle proof
        proofs[0].chainId = uint64(block.chainid + 2);
        proofs[0].blockId = 1;
        proofs[0].rootHash = randBytes32();
        proofs[0].accountProof = new bytes[](1);
        proofs[0].storageProof = new bytes[](10);

        // second hop with storage merkle proof
        proofs[1].chainId = uint64(block.chainid + 3);
        proofs[1].blockId = 2;
        proofs[1].rootHash = randBytes32();
        proofs[1].accountProof = new bytes[](0);
        proofs[1].storageProof = new bytes[](10);

        // third/last hop with full merkle proof
        proofs[2].chainId = uint64(block.chainid);
        proofs[2].blockId = 3;
        proofs[2].rootHash = randBytes32();
        proofs[2].accountProof = new bytes[](1);
        proofs[2].storageProof = new bytes[](10);

        // expect RESOLVER_ZERO_ADDR
        vm.expectRevert(
            abi.encodeWithSelector(
                AddressResolver.RESOLVER_ZERO_ADDR.selector,
                proofs[0].chainId,
                strToBytes32("signal_service")
            )
        );
        signalService.proveSignalReceived({
            _chainId: srcChainId,
            _app: randAddress(),
            _signal: randBytes32(),
            _proof: abi.encode(proofs)
        });

        // Add two trusted hop relayers
        vm.startPrank(Alice);
        addressManager.setAddress(proofs[0].chainId, "signal_service", randAddress() /*relay1*/ );
        addressManager.setAddress(proofs[1].chainId, "signal_service", randAddress() /*relay2*/ );
        vm.stopPrank();

        vm.expectRevert(SignalService.SS_SIGNAL_NOT_FOUND.selector);
        signalService.proveSignalReceived({
            _chainId: srcChainId,
            _app: randAddress(),
            _signal: randBytes32(),
            _proof: abi.encode(proofs)
        });

        vm.prank(taiko);
        signalService.syncChainData(
            proofs[1].chainId, LibStrings.H_STATE_ROOT, proofs[2].blockId, proofs[2].rootHash
        );

        signalService.proveSignalReceived({
            _chainId: srcChainId,
            _app: randAddress(),
            _signal: randBytes32(),
            _proof: abi.encode(proofs)
        });
    }

    function test_SignalService_proveSignalReceived_revert_with_a_loop() public {
        uint64 srcChainId = uint64(block.chainid + 1);

        vm.prank(Alice);
        addressManager.setAddress(srcChainId, "signal_service", randAddress());

        SignalService.HopProof[] memory proofs = new SignalService.HopProof[](3);

        // first hop with full merkle proof
        proofs[0].chainId = uint64(block.chainid + 2);
        proofs[0].blockId = 1;
        proofs[0].rootHash = randBytes32();
        proofs[0].accountProof = new bytes[](1);
        proofs[0].storageProof = new bytes[](10);

        // second hop with storage merkle proof
        proofs[1].chainId = proofs[0].chainId; // same
        proofs[1].blockId = 2;
        proofs[1].rootHash = randBytes32();
        proofs[1].accountProof = new bytes[](0);
        proofs[1].storageProof = new bytes[](10);

        // third/last hop with full merkle proof
        proofs[2].chainId = uint64(block.chainid);
        proofs[2].blockId = 3;
        proofs[2].rootHash = randBytes32();
        proofs[2].accountProof = new bytes[](1);
        proofs[2].storageProof = new bytes[](10);

        // Add two trusted hop relayers
        vm.startPrank(Alice);
        addressManager.setAddress(proofs[0].chainId, "signal_service", randAddress() /*relay1*/ );
        addressManager.setAddress(proofs[1].chainId, "signal_service", randAddress() /*relay2*/ );
        vm.stopPrank();

        vm.prank(taiko);
        signalService.syncChainData(
            proofs[1].chainId, LibStrings.H_STATE_ROOT, proofs[2].blockId, proofs[2].rootHash
        );

        vm.expectRevert(SignalService.SS_INVALID_HOPS_WITH_LOOP.selector);
        signalService.proveSignalReceived({
            _chainId: srcChainId,
            _app: randAddress(),
            _signal: randBytes32(),
            _proof: abi.encode(proofs)
        });
    }

    function test_SignalService_proveSignalReceived_multiple_hops_caching() public {
        uint64 srcChainId = uint64(block.chainid + 1);
        uint64 nextChainId = srcChainId + 100;

        SignalService.HopProof[] memory proofs = new SignalService.HopProof[](9);

        // hop 1:  full merkle proof, CACHE_NOTHING
        proofs[0].chainId = nextChainId++;
        proofs[0].blockId = 1;
        proofs[0].rootHash = randBytes32();
        proofs[0].accountProof = new bytes[](1);
        proofs[0].storageProof = new bytes[](10);
        proofs[0].cacheOption = ISignalService.CacheOption.CACHE_NOTHING;

        // hop 2:  full merkle proof, CACHE_STATE_ROOT
        proofs[1].chainId = nextChainId++;
        proofs[1].blockId = 2;
        proofs[1].rootHash = randBytes32();
        proofs[1].accountProof = new bytes[](1);
        proofs[1].storageProof = new bytes[](10);
        proofs[1].cacheOption = ISignalService.CacheOption.CACHE_STATE_ROOT;

        // hop 3:  full merkle proof, CACHE_SIGNAL_ROOT
        proofs[2].chainId = nextChainId++;
        proofs[2].blockId = 3;
        proofs[2].rootHash = randBytes32();
        proofs[2].accountProof = new bytes[](1);
        proofs[2].storageProof = new bytes[](10);
        proofs[2].cacheOption = ISignalService.CacheOption.CACHE_SIGNAL_ROOT;

        // hop 4:  full merkle proof, CACHE_BOTH
        proofs[3].chainId = nextChainId++;
        proofs[3].blockId = 4;
        proofs[3].rootHash = randBytes32();
        proofs[3].accountProof = new bytes[](1);
        proofs[3].storageProof = new bytes[](10);
        proofs[3].cacheOption = ISignalService.CacheOption.CACHE_BOTH;

        // hop 5:  storage merkle proof, CACHE_NOTHING
        proofs[4].chainId = nextChainId++;
        proofs[4].blockId = 5;
        proofs[4].rootHash = randBytes32();
        proofs[4].accountProof = new bytes[](0);
        proofs[4].storageProof = new bytes[](10);
        proofs[4].cacheOption = ISignalService.CacheOption.CACHE_NOTHING;

        // hop 6:  storage merkle proof, CACHE_STATE_ROOT
        proofs[5].chainId = nextChainId++;
        proofs[5].blockId = 6;
        proofs[5].rootHash = randBytes32();
        proofs[5].accountProof = new bytes[](0);
        proofs[5].storageProof = new bytes[](10);
        proofs[5].cacheOption = ISignalService.CacheOption.CACHE_STATE_ROOT;

        // hop 7:  storage merkle proof, CACHE_SIGNAL_ROOT
        proofs[6].chainId = nextChainId++;
        proofs[6].blockId = 7;
        proofs[6].rootHash = randBytes32();
        proofs[6].accountProof = new bytes[](0);
        proofs[6].storageProof = new bytes[](10);
        proofs[6].cacheOption = ISignalService.CacheOption.CACHE_SIGNAL_ROOT;

        // hop 8:  storage merkle proof, CACHE_BOTH
        proofs[7].chainId = nextChainId++;
        proofs[7].blockId = 8;
        proofs[7].rootHash = randBytes32();
        proofs[7].accountProof = new bytes[](0);
        proofs[7].storageProof = new bytes[](10);
        proofs[7].cacheOption = ISignalService.CacheOption.CACHE_BOTH;

        // last hop, 9:  full merkle proof, CACHE_BOTH
        proofs[8].chainId = uint64(block.chainid);
        proofs[8].blockId = 9;
        proofs[8].rootHash = randBytes32();
        proofs[8].accountProof = new bytes[](1);
        proofs[8].storageProof = new bytes[](10);
        proofs[8].cacheOption = ISignalService.CacheOption.CACHE_BOTH;

        // Add two trusted hop relayers
        vm.startPrank(Alice);
        addressManager.setAddress(srcChainId, "signal_service", randAddress());
        for (uint256 i; i < proofs.length; ++i) {
            addressManager.setAddress(
                proofs[i].chainId, "signal_service", randAddress() /*relay1*/
            );
        }
        vm.stopPrank();

        vm.prank(taiko);
        signalService.syncChainData(
            proofs[7].chainId, LibStrings.H_STATE_ROOT, proofs[8].blockId, proofs[8].rootHash
        );

        signalService.proveSignalReceived({
            _chainId: srcChainId,
            _app: randAddress(),
            _signal: randBytes32(),
            _proof: abi.encode(proofs)
        });

        // hop 1:  full merkle proof, CACHE_NOTHING
        _verifyCache(srcChainId, proofs[0].blockId, proofs[0].rootHash, false, false);
        // hop 2:  full merkle proof, CACHE_STATE_ROOT
        _verifyCache(proofs[0].chainId, proofs[1].blockId, proofs[1].rootHash, true, false);
        // hop 3:  full merkle proof, CACHE_SIGNAL_ROOT
        _verifyCache(proofs[1].chainId, proofs[2].blockId, proofs[2].rootHash, false, true);
        // hop 4:  full merkle proof, CACHE_BOTH
        _verifyCache(proofs[2].chainId, proofs[3].blockId, proofs[3].rootHash, true, true);
        // hop 5:  storage merkle proof, CACHE_NOTHING
        _verifyCache(proofs[3].chainId, proofs[4].blockId, proofs[4].rootHash, false, false);
        // hop 6:  storage merkle proof, CACHE_STATE_ROOT
        _verifyCache(proofs[4].chainId, proofs[5].blockId, proofs[5].rootHash, false, false);
        // hop 7:  storage merkle proof, CACHE_SIGNAL_ROOT
        _verifyCache(proofs[5].chainId, proofs[6].blockId, proofs[6].rootHash, false, true);
        // hop 8:  storage merkle proof, CACHE_BOTH
        _verifyCache(proofs[6].chainId, proofs[7].blockId, proofs[7].rootHash, false, true);
        // last hop, 9:  full merkle proof, CACHE_BOTH
        // last hop's state root is already cached even before the proveSignalReceived call.
        _verifyCache(proofs[7].chainId, proofs[8].blockId, proofs[8].rootHash, true, true);
    }

    function _verifyCache(
        uint64 chainId,
        uint64 blockId,
        bytes32 stateRoot,
        bool stateRootCached,
        bool signalRootCached
    )
        private
    {
        assertEq(
            signalService.isChainDataSynced(chainId, LibStrings.H_STATE_ROOT, blockId, stateRoot),
            stateRootCached
        );

        assertEq(
            signalService.isChainDataSynced(
                chainId, LibStrings.H_SIGNAL_ROOT, blockId, bytes32(uint256(789))
            ),
            signalRootCached
        );
    }
}
