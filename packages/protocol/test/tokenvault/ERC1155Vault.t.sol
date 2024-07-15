// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import "@openzeppelin/contracts/token/ERC1155/ERC1155.sol";
import "../TaikoTest.sol";

contract TestTokenERC1155 is ERC1155 {
    constructor(string memory baseURI) ERC1155(baseURI) { }

    function mint(uint256 tokenId, uint256 amount) public {
        _mint(msg.sender, tokenId, amount, "");
    }
}

// PrankDestBridge lets us simulate a transaction to the ERC1155Vault
// from a named Bridge, without having to test/run through the real Bridge code,
// outside the scope of the unit tests in the ERC1155Vault.
contract PrankDestBridge {
    ERC1155Vault destERC1155Vault;

    struct BridgeContext {
        bytes32 msgHash;
        address sender;
        uint64 srcChainId;
    }

    BridgeContext ctx;

    constructor(ERC1155Vault _erc1155Vault) {
        destERC1155Vault = _erc1155Vault;
    }

    function setERC1155Vault(address addr) public {
        destERC1155Vault = ERC1155Vault(addr);
    }

    function sendMessage(IBridge.Message memory message)
        external
        payable
        returns (bytes32 msgHash, IBridge.Message memory _message)
    {
        // Dummy return value
        return (keccak256(abi.encode(message.id)), _message);
    }

    function context() public view returns (BridgeContext memory) {
        return ctx;
    }

    function sendReceiveERC1155ToERC1155Vault(
        BaseNFTVault.CanonicalNFT calldata ctoken,
        address from,
        address to,
        uint256[] memory tokenIds,
        uint256[] memory amounts,
        bytes32 msgHash,
        address srcChainERC1155Vault,
        uint64 srcChainId,
        uint256 mockLibInvokeMsgValue
    )
        public
    {
        ctx.sender = srcChainERC1155Vault;
        ctx.msgHash = msgHash;
        ctx.srcChainId = srcChainId;

        // We need this in order to 'mock' the LibBridgeInvoke's
        //  (success,retVal) =
        //     message.to.call{ value: message.value, gas: gasLimit
        // }(message.data);
        // The problem (with foundry) is that this way it is not able to deploy
        // a contract
        // most probably due to some deployment address nonce issue. (Seems a
        // known issue).
        destERC1155Vault.onMessageInvocation{ value: mockLibInvokeMsgValue }(
            abi.encode(ctoken, from, to, tokenIds, amounts)
        );

        ctx.sender = address(0);
        ctx.msgHash = bytes32(0);
        ctx.srcChainId = 0;
    }
}

contract UpdatedBridgedERC1155 is BridgedERC1155 {
    function helloWorld() public pure returns (string memory) {
        return "helloworld";
    }
}

contract ERC1155VaultTest is TaikoTest {
    uint32 private constant GAS_LIMIT = 2_000_000;
    AddressManager addressManager;
    BadReceiver badReceiver;
    Bridge bridge;
    Bridge destChainBridge;
    PrankDestBridge destChainIdBridge;
    SkipProofCheckSignal mockProofSignalService;
    ERC1155Vault erc1155Vault;
    ERC1155Vault destChainErc1155Vault;
    TestTokenERC1155 ctoken1155;
    SignalService signalService;
    uint64 destChainId = 19_389;

    function setUp() public {
        vm.startPrank(Carol);
        vm.deal(Alice, 100 ether);
        vm.deal(Carol, 100 ether);
        vm.deal(Bob, 100 ether);
        addressManager = AddressManager(
            deployProxy({
                name: "address_manager",
                impl: address(new AddressManager()),
                data: abi.encodeCall(AddressManager.init, (address(0)))
            })
        );

        bridge = Bridge(
            payable(
                deployProxy({
                    name: "bridge",
                    impl: address(new Bridge()),
                    data: abi.encodeCall(Bridge.init, (address(0), address(addressManager))),
                    registerTo: address(addressManager)
                })
            )
        );

        destChainBridge = Bridge(
            payable(
                deployProxy({
                    name: "bridge",
                    impl: address(new Bridge()),
                    data: abi.encodeCall(Bridge.init, (address(0), address(addressManager))),
                    registerTo: address(addressManager)
                })
            )
        );

        signalService = SignalService(
            deployProxy({
                name: "signal_service",
                impl: address(new SignalService()),
                data: abi.encodeCall(SignalService.init, (address(0), address(addressManager)))
            })
        );

        erc1155Vault = ERC1155Vault(
            deployProxy({
                name: "erc1155_vault",
                impl: address(new ERC1155Vault()),
                data: abi.encodeCall(ERC1155Vault.init, (address(0), address(addressManager)))
            })
        );

        destChainErc1155Vault = ERC1155Vault(
            deployProxy({
                name: "erc1155_vault",
                impl: address(new ERC1155Vault()),
                data: abi.encodeCall(ERC1155Vault.init, (address(0), address(addressManager)))
            })
        );

        destChainIdBridge = new PrankDestBridge(destChainErc1155Vault);
        vm.deal(address(destChainIdBridge), 100 ether);

        mockProofSignalService = SkipProofCheckSignal(
            deployProxy({
                name: "signal_service",
                impl: address(new SkipProofCheckSignal()),
                data: abi.encodeCall(SignalService.init, (address(0), address(addressManager)))
            })
        );

        addressManager.setAddress(
            uint64(block.chainid), "signal_service", address(mockProofSignalService)
        );

        addressManager.setAddress(destChainId, "signal_service", address(mockProofSignalService));

        addressManager.setAddress(uint64(block.chainid), "bridge", address(bridge));

        addressManager.setAddress(destChainId, "bridge", address(destChainIdBridge));

        addressManager.setAddress(uint64(block.chainid), "erc1155_vault", address(erc1155Vault));

        addressManager.setAddress(destChainId, "erc1155_vault", address(destChainErc1155Vault));

        // Below 2-2 registrations (mock) are needed bc of
        // LibBridgeRecall.sol's
        // resolve address
        addressManager.setAddress(destChainId, "erc721_vault", address(mockProofSignalService));
        addressManager.setAddress(destChainId, "erc20_vault", address(mockProofSignalService));
        addressManager.setAddress(
            uint64(block.chainid), "erc721_vault", address(mockProofSignalService)
        );
        addressManager.setAddress(
            uint64(block.chainid), "erc20_vault", address(mockProofSignalService)
        );

        vm.deal(address(bridge), 100 ether);

        address bridgedERC1155 = address(new BridgedERC1155());

        addressManager.setAddress(destChainId, "bridged_erc1155", bridgedERC1155);
        addressManager.setAddress(uint64(block.chainid), "bridged_erc1155", bridgedERC1155);

        ctoken1155 = new TestTokenERC1155("http://example.host.com/");
        vm.stopPrank();
        vm.startPrank(Alice, Alice);
        ctoken1155.mint(1, 10);
        ctoken1155.mint(2, 10);

        vm.stopPrank();
    }

    function getPreDeterminedDataBytes() internal pure returns (bytes memory) {
        return
        hex"20b8155900000000000000000000000000000000000000000000000000000000000000a00000000000000000000000007e5f4552091a69125d5dfcb7b8c2659029395bdf0000000000000000000000007e5f4552091a69125d5dfcb7b8c2659029395bdf000000000000000000000000000000000000000000000000000000000000016000000000000000000000000000000000000000000000000000000000000001a00000000000000000000000000000000000000000000000000000000000007a690000000000000000000000007935de70183a080242a58f64637a8e7f15349b63000000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000000000a0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000000000000000000000002";
    }

    function test_1155Vault_sendToken_1155() public {
        vm.prank(Alice, Alice);
        ctoken1155.setApprovalForAll(address(erc1155Vault), true);

        assertEq(ctoken1155.balanceOf(Alice, 1), 10);
        assertEq(ctoken1155.balanceOf(address(erc1155Vault), 1), 0);

        uint256[] memory tokenIds = new uint256[](1);
        tokenIds[0] = 1;

        uint256[] memory amounts = new uint256[](1);
        amounts[0] = 2;

        BaseNFTVault.BridgeTransferOp memory sendOpts = BaseNFTVault.BridgeTransferOp(
            destChainId,
            address(0),
            Alice,
            GAS_LIMIT,
            address(ctoken1155),
            GAS_LIMIT,
            tokenIds,
            amounts
        );
        vm.prank(Alice, Alice);
        erc1155Vault.sendToken{ value: GAS_LIMIT }(sendOpts);

        assertEq(ctoken1155.balanceOf(Alice, 1), 8);
        assertEq(ctoken1155.balanceOf(address(erc1155Vault), 1), 2);
    }

    function test_1155Vault_sendToken_with_invalid_token_address_1155() public {
        vm.prank(Alice, Alice);
        ctoken1155.setApprovalForAll(address(erc1155Vault), true);

        assertEq(ctoken1155.balanceOf(Alice, 1), 10);
        assertEq(ctoken1155.balanceOf(address(erc1155Vault), 1), 0);

        uint256[] memory tokenIds = new uint256[](1);
        tokenIds[0] = 1;

        uint256[] memory amounts = new uint256[](1);
        amounts[0] = 2;

        BaseNFTVault.BridgeTransferOp memory sendOpts = BaseNFTVault.BridgeTransferOp(
            destChainId, address(0), Alice, GAS_LIMIT, address(0), GAS_LIMIT, tokenIds, amounts
        );
        vm.prank(Alice, Alice);
        vm.expectRevert(BaseNFTVault.VAULT_INVALID_TOKEN.selector);
        erc1155Vault.sendToken{ value: GAS_LIMIT }(sendOpts);
    }

    function test_1155Vault_sendToken_with_0_tokens_1155() public {
        vm.prank(Alice, Alice);
        ctoken1155.setApprovalForAll(address(erc1155Vault), true);

        assertEq(ctoken1155.balanceOf(Alice, 1), 10);
        assertEq(ctoken1155.balanceOf(address(erc1155Vault), 1), 0);

        uint256[] memory tokenIds = new uint256[](1);
        tokenIds[0] = 1;

        uint256[] memory amounts = new uint256[](1);
        amounts[0] = 0;

        BaseNFTVault.BridgeTransferOp memory sendOpts = BaseNFTVault.BridgeTransferOp(
            destChainId,
            address(0),
            Alice,
            GAS_LIMIT,
            address(ctoken1155),
            GAS_LIMIT,
            tokenIds,
            amounts
        );
        vm.prank(Alice, Alice);
        vm.expectRevert(BaseNFTVault.VAULT_INVALID_AMOUNT.selector);
        erc1155Vault.sendToken{ value: GAS_LIMIT }(sendOpts);
    }

    function test_1155Vault_receiveTokens_from_newly_deployed_bridged_contract_on_destination_chain_1155(
    )
        public
    {
        vm.prank(Alice, Alice);
        ctoken1155.setApprovalForAll(address(erc1155Vault), true);

        assertEq(ctoken1155.balanceOf(Alice, 1), 10);
        assertEq(ctoken1155.balanceOf(address(erc1155Vault), 1), 0);

        uint256[] memory tokenIds = new uint256[](1);
        tokenIds[0] = 1;

        uint256[] memory amounts = new uint256[](1);
        amounts[0] = 2;

        BaseNFTVault.BridgeTransferOp memory sendOpts = BaseNFTVault.BridgeTransferOp(
            destChainId,
            address(0),
            Alice,
            GAS_LIMIT,
            address(ctoken1155),
            GAS_LIMIT,
            tokenIds,
            amounts
        );
        vm.prank(Alice, Alice);
        erc1155Vault.sendToken{ value: GAS_LIMIT }(sendOpts);

        assertEq(ctoken1155.balanceOf(Alice, 1), 8);
        assertEq(ctoken1155.balanceOf(address(erc1155Vault), 1), 2);

        amounts[0] = 2;
        BaseNFTVault.CanonicalNFT memory ctoken = BaseNFTVault.CanonicalNFT({
            chainId: 31_337,
            addr: address(ctoken1155),
            symbol: "",
            name: ""
        });

        uint64 srcChainId = uint64(block.chainid);
        vm.chainId(destChainId);

        destChainIdBridge.sendReceiveERC1155ToERC1155Vault(
            ctoken,
            Alice,
            Alice,
            tokenIds,
            amounts,
            bytes32(0),
            address(erc1155Vault),
            srcChainId,
            0
        );

        // Query canonicalToBridged
        address deployedContract =
            destChainErc1155Vault.canonicalToBridged(srcChainId, address(ctoken1155));

        // Alice bridged over 2 items
        assertEq(ERC1155(deployedContract).balanceOf(Alice, 1), 2);
    }

    function test_1155Vault_receiveTokens_but_mint_not_deploy_if_bridged_second_time_1155()
        public
    {
        vm.prank(Alice, Alice);
        ctoken1155.setApprovalForAll(address(erc1155Vault), true);

        assertEq(ctoken1155.balanceOf(Alice, 1), 10);
        assertEq(ctoken1155.balanceOf(address(erc1155Vault), 1), 0);

        uint256[] memory tokenIds = new uint256[](1);
        tokenIds[0] = 1;

        uint256[] memory amounts = new uint256[](1);
        amounts[0] = 2;

        BaseNFTVault.BridgeTransferOp memory sendOpts = BaseNFTVault.BridgeTransferOp(
            destChainId,
            address(0),
            Alice,
            GAS_LIMIT,
            address(ctoken1155),
            GAS_LIMIT,
            tokenIds,
            amounts
        );
        vm.prank(Alice, Alice);
        erc1155Vault.sendToken{ value: GAS_LIMIT }(sendOpts);

        assertEq(ctoken1155.balanceOf(Alice, 1), 8);
        assertEq(ctoken1155.balanceOf(address(erc1155Vault), 1), 2);

        BaseNFTVault.CanonicalNFT memory ctoken = BaseNFTVault.CanonicalNFT({
            chainId: 31_337,
            addr: address(ctoken1155),
            symbol: "",
            name: ""
        });

        uint64 srcChainId = uint64(block.chainid);
        vm.chainId(destChainId);

        destChainIdBridge.sendReceiveERC1155ToERC1155Vault(
            ctoken,
            Alice,
            Alice,
            tokenIds,
            amounts,
            bytes32(0),
            address(erc1155Vault),
            srcChainId,
            0
        );

        // Query canonicalToBridged
        address deployedContract =
            destChainErc1155Vault.canonicalToBridged(srcChainId, address(ctoken1155));

        // Alice bridged over 2 items
        assertEq(ERC1155(deployedContract).balanceOf(Alice, 1), 2);

        // Change back to 'L1'
        vm.chainId(srcChainId);

        tokenIds[0] = 1;
        amounts[0] = 1;

        sendOpts = BaseNFTVault.BridgeTransferOp(
            destChainId,
            address(0),
            Alice,
            GAS_LIMIT,
            address(ctoken1155),
            GAS_LIMIT,
            tokenIds,
            amounts
        );
        vm.prank(Alice, Alice);
        erc1155Vault.sendToken{ value: GAS_LIMIT }(sendOpts);

        assertEq(ctoken1155.balanceOf(Alice, 1), 7);
        assertEq(ctoken1155.balanceOf(address(erc1155Vault), 1), 3);

        vm.chainId(destChainId);

        destChainIdBridge.sendReceiveERC1155ToERC1155Vault(
            ctoken,
            Alice,
            Alice,
            tokenIds,
            amounts,
            bytes32(0),
            address(erc1155Vault),
            srcChainId,
            0
        );

        // Query canonicalToBridged
        address bridgedContract =
            destChainErc1155Vault.canonicalToBridged(srcChainId, address(ctoken1155));

        assertEq(bridgedContract, deployedContract);
    }

    function test_1155Vault_receiveTokens_erc1155_with_ether_to_dave() public {
        vm.prank(Alice, Alice);
        ctoken1155.setApprovalForAll(address(erc1155Vault), true);

        assertEq(ctoken1155.balanceOf(Alice, 1), 10);
        assertEq(ctoken1155.balanceOf(address(erc1155Vault), 1), 0);

        uint256[] memory tokenIds = new uint256[](1);
        tokenIds[0] = 1;

        uint256[] memory amounts = new uint256[](1);
        amounts[0] = 2;

        uint256 etherValue = 0.1 ether;

        BaseNFTVault.BridgeTransferOp memory sendOpts = BaseNFTVault.BridgeTransferOp(
            destChainId,
            address(0),
            David,
            GAS_LIMIT,
            address(ctoken1155),
            GAS_LIMIT,
            tokenIds,
            amounts
        );
        vm.prank(Alice, Alice);
        erc1155Vault.sendToken{ value: etherValue }(sendOpts);

        assertEq(ctoken1155.balanceOf(Alice, 1), 8);
        assertEq(ctoken1155.balanceOf(address(erc1155Vault), 1), 2);

        amounts[0] = 2;
        BaseNFTVault.CanonicalNFT memory ctoken = BaseNFTVault.CanonicalNFT({
            chainId: 31_337,
            addr: address(ctoken1155),
            symbol: "",
            name: ""
        });

        uint64 srcChainId = uint64(block.chainid);
        vm.chainId(destChainId);

        destChainIdBridge.sendReceiveERC1155ToERC1155Vault(
            ctoken,
            Alice,
            David,
            tokenIds,
            amounts,
            bytes32(0),
            address(erc1155Vault),
            srcChainId,
            etherValue
        );

        // Query canonicalToBridged
        address deployedContract =
            destChainErc1155Vault.canonicalToBridged(srcChainId, address(ctoken1155));

        // Alice bridged over 2 items and etherValue to David
        assertEq(ERC1155(deployedContract).balanceOf(David, 1), 2);
        assertEq(David.balance, etherValue);
    }

    function test_1155Vault_onMessageRecalled_1155() public {
        vm.prank(Alice, Alice);
        ctoken1155.setApprovalForAll(address(erc1155Vault), true);

        assertEq(ctoken1155.balanceOf(Alice, 1), 10);
        assertEq(ctoken1155.balanceOf(address(erc1155Vault), 1), 0);

        uint256[] memory tokenIds = new uint256[](1);
        tokenIds[0] = 1;

        uint256[] memory amounts = new uint256[](1);
        amounts[0] = 2;

        BaseNFTVault.BridgeTransferOp memory sendOpts = BaseNFTVault.BridgeTransferOp(
            destChainId,
            address(0),
            Alice,
            GAS_LIMIT,
            address(ctoken1155),
            GAS_LIMIT,
            tokenIds,
            amounts
        );

        vm.prank(Alice, Alice);
        IBridge.Message memory message = erc1155Vault.sendToken{ value: GAS_LIMIT }(sendOpts);

        assertEq(ctoken1155.balanceOf(Alice, 1), 8);
        assertEq(ctoken1155.balanceOf(address(erc1155Vault), 1), 2);

        bridge.recallMessage(message, bytes(""));

        // Alice got back her NFTs, and vault has 0
        assertEq(ctoken1155.balanceOf(Alice, 1), 10);
        assertEq(ctoken1155.balanceOf(address(erc1155Vault), 1), 0);
    }

    function test_1155Vault_receiveTokens_multiple_1155() public {
        vm.prank(Alice, Alice);
        ctoken1155.setApprovalForAll(address(erc1155Vault), true);

        assertEq(ctoken1155.balanceOf(Alice, 1), 10);
        assertEq(ctoken1155.balanceOf(address(erc1155Vault), 1), 0);

        assertEq(ctoken1155.balanceOf(Alice, 2), 10);
        assertEq(ctoken1155.balanceOf(address(erc1155Vault), 2), 0);

        uint256[] memory tokenIds = new uint256[](2);
        tokenIds[0] = 1;
        tokenIds[1] = 2;

        uint256[] memory amounts = new uint256[](2);
        amounts[0] = 2;
        amounts[1] = 5;

        BaseNFTVault.BridgeTransferOp memory sendOpts = BaseNFTVault.BridgeTransferOp(
            destChainId,
            address(0),
            Alice,
            GAS_LIMIT,
            address(ctoken1155),
            GAS_LIMIT,
            tokenIds,
            amounts
        );
        vm.prank(Alice, Alice);
        erc1155Vault.sendToken{ value: GAS_LIMIT }(sendOpts);

        assertEq(ctoken1155.balanceOf(Alice, 1), 8);
        assertEq(ctoken1155.balanceOf(address(erc1155Vault), 1), 2);

        assertEq(ctoken1155.balanceOf(Alice, 2), 5);
        assertEq(ctoken1155.balanceOf(address(erc1155Vault), 2), 5);

        BaseNFTVault.CanonicalNFT memory ctoken = BaseNFTVault.CanonicalNFT({
            chainId: 31_337,
            addr: address(ctoken1155),
            symbol: "",
            name: ""
        });

        uint64 srcChainId = uint64(block.chainid);
        vm.chainId(destChainId);

        destChainIdBridge.sendReceiveERC1155ToERC1155Vault(
            ctoken,
            Alice,
            Alice,
            tokenIds,
            amounts,
            bytes32(0),
            address(erc1155Vault),
            srcChainId,
            0
        );

        // Query canonicalToBridged
        address deployedContract =
            destChainErc1155Vault.canonicalToBridged(srcChainId, address(ctoken1155));

        // Alice bridged over 2 items
        assertEq(ERC1155(deployedContract).balanceOf(Alice, 1), 2);
        assertEq(ERC1155(deployedContract).balanceOf(Alice, 2), 5);
    }

    function test_1155Vault_bridge_back_but_owner_is_different_now_1155() public {
        vm.prank(Alice, Alice);
        ctoken1155.setApprovalForAll(address(erc1155Vault), true);

        assertEq(ctoken1155.balanceOf(Alice, 1), 10);
        assertEq(ctoken1155.balanceOf(address(erc1155Vault), 1), 0);

        uint256[] memory tokenIds = new uint256[](1);
        tokenIds[0] = 1;

        uint256[] memory amounts = new uint256[](1);
        amounts[0] = 1;

        BaseNFTVault.BridgeTransferOp memory sendOpts = BaseNFTVault.BridgeTransferOp(
            destChainId,
            address(0),
            Alice,
            GAS_LIMIT,
            address(ctoken1155),
            GAS_LIMIT,
            tokenIds,
            amounts
        );
        vm.prank(Alice, Alice);
        erc1155Vault.sendToken{ value: GAS_LIMIT }(sendOpts);

        assertEq(ctoken1155.balanceOf(address(erc1155Vault), 1), 1);

        // This canonicalToken is basically need to be exact same as the
        // sendToken() puts together
        // - here is just mocking putting it together.
        BaseNFTVault.CanonicalNFT memory canonicalToken = BaseNFTVault.CanonicalNFT({
            chainId: 31_337,
            addr: address(ctoken1155),
            symbol: "TT",
            name: "TT"
        });

        uint64 chainId = uint64(block.chainid);
        vm.chainId(destChainId);

        destChainIdBridge.sendReceiveERC1155ToERC1155Vault(
            canonicalToken,
            Alice,
            Alice,
            tokenIds,
            amounts,
            bytes32(0),
            address(erc1155Vault),
            chainId,
            0
        );
        // Query canonicalToBridged
        address deployedContract =
            destChainErc1155Vault.canonicalToBridged(chainId, address(ctoken1155));

        // Alice bridged over 1 from tokenId 1
        assertEq(ERC1155(deployedContract).balanceOf(Alice, 1), 1);

        // Transfer the asset to Bob, and Bob can receive it back on canonical
        // chain
        vm.prank(Alice, Alice);
        ERC1155(deployedContract).safeTransferFrom(Alice, Bob, 1, 1, "");

        assertEq(ERC1155(deployedContract).balanceOf(Bob, 1), 1);
        assertEq(ERC1155(deployedContract).balanceOf(Alice, 1), 0);

        vm.prank(Bob, Bob);
        ERC1155(deployedContract).setApprovalForAll(address(destChainErc1155Vault), true);

        sendOpts = BaseNFTVault.BridgeTransferOp(
            chainId,
            address(0),
            Bob,
            GAS_LIMIT,
            address(deployedContract),
            GAS_LIMIT,
            tokenIds,
            amounts
        );

        vm.prank(Bob, Bob);
        destChainErc1155Vault.sendToken{ value: GAS_LIMIT }(sendOpts);

        vm.chainId(chainId);

        assertEq(ctoken1155.balanceOf(address(erc1155Vault), 1), 1);

        destChainIdBridge.setERC1155Vault(address(erc1155Vault));

        vm.prank(Carol, Carol);
        addressManager.setAddress(uint64(block.chainid), "bridge", address(destChainIdBridge));

        destChainIdBridge.sendReceiveERC1155ToERC1155Vault(
            canonicalToken,
            Bob,
            Bob,
            tokenIds,
            amounts,
            bytes32(0),
            address(erc1155Vault),
            chainId,
            0
        );

        assertEq(ctoken1155.balanceOf(Bob, 1), 1);
    }

    function test_1155Vault_bridge_back_but_original_owner_cannot_claim_it_anymore_if_sold_1155()
        public
    {
        vm.prank(Alice, Alice);
        ctoken1155.setApprovalForAll(address(erc1155Vault), true);

        assertEq(ctoken1155.balanceOf(Alice, 1), 10);
        assertEq(ctoken1155.balanceOf(address(erc1155Vault), 1), 0);

        uint256[] memory tokenIds = new uint256[](1);
        tokenIds[0] = 1;

        uint256[] memory amounts = new uint256[](1);
        amounts[0] = 1;

        BaseNFTVault.BridgeTransferOp memory sendOpts = BaseNFTVault.BridgeTransferOp(
            destChainId,
            address(0),
            Alice,
            GAS_LIMIT,
            address(ctoken1155),
            GAS_LIMIT,
            tokenIds,
            amounts
        );
        vm.prank(Alice, Alice);
        erc1155Vault.sendToken{ value: GAS_LIMIT }(sendOpts);

        assertEq(ctoken1155.balanceOf(address(erc1155Vault), 1), 1);

        // This canonicalToken is basically need to be exact same as the
        // sendToken() puts together
        // - here is just mocking putting it together.
        BaseNFTVault.CanonicalNFT memory canonicalToken = BaseNFTVault.CanonicalNFT({
            chainId: 31_337,
            addr: address(ctoken1155),
            symbol: "TT",
            name: "TT"
        });

        uint64 chainId = uint64(block.chainid);
        vm.chainId(destChainId);

        destChainIdBridge.sendReceiveERC1155ToERC1155Vault(
            canonicalToken,
            Alice,
            Alice,
            tokenIds,
            amounts,
            bytes32(0),
            address(erc1155Vault),
            chainId,
            0
        );

        // Query canonicalToBridged
        address deployedContract =
            destChainErc1155Vault.canonicalToBridged(chainId, address(ctoken1155));
        // Alice bridged over 1 from tokenId 1
        assertEq(ERC1155(deployedContract).balanceOf(Alice, 1), 1);

        // Transfer the asset to Bob, and Bob can receive it back on canonical
        // chain
        vm.prank(Alice, Alice);
        ERC1155(deployedContract).safeTransferFrom(Alice, Bob, 1, 1, "");

        assertEq(ERC1155(deployedContract).balanceOf(Bob, 1), 1);
        assertEq(ERC1155(deployedContract).balanceOf(Alice, 1), 0);

        vm.prank(Bob, Bob);
        ERC1155(deployedContract).setApprovalForAll(address(destChainErc1155Vault), true);

        sendOpts = BaseNFTVault.BridgeTransferOp(
            chainId,
            address(0),
            Alice,
            GAS_LIMIT,
            address(deployedContract),
            GAS_LIMIT,
            tokenIds,
            amounts
        );

        vm.prank(Alice, Alice);
        vm.expectRevert("ERC1155: caller is not token owner or approved");
        destChainErc1155Vault.sendToken{ value: GAS_LIMIT }(sendOpts);
    }

    function test_1155Vault_upgrade_bridged_tokens_1155() public {
        vm.prank(Alice, Alice);
        ctoken1155.setApprovalForAll(address(erc1155Vault), true);

        assertEq(ctoken1155.balanceOf(Alice, 1), 10);
        assertEq(ctoken1155.balanceOf(address(erc1155Vault), 1), 0);

        uint256[] memory tokenIds = new uint256[](1);
        tokenIds[0] = 1;

        uint256[] memory amounts = new uint256[](1);
        amounts[0] = 2;

        BaseNFTVault.BridgeTransferOp memory sendOpts = BaseNFTVault.BridgeTransferOp(
            destChainId,
            address(0),
            Alice,
            GAS_LIMIT,
            address(ctoken1155),
            GAS_LIMIT,
            tokenIds,
            amounts
        );
        vm.prank(Alice, Alice);
        erc1155Vault.sendToken{ value: GAS_LIMIT }(sendOpts);

        assertEq(ctoken1155.balanceOf(Alice, 1), 8);
        assertEq(ctoken1155.balanceOf(address(erc1155Vault), 1), 2);

        BaseNFTVault.CanonicalNFT memory ctoken = BaseNFTVault.CanonicalNFT({
            chainId: 31_337,
            addr: address(ctoken1155),
            symbol: "",
            name: ""
        });

        uint64 srcChainId = uint64(block.chainid);
        vm.chainId(destChainId);

        destChainIdBridge.sendReceiveERC1155ToERC1155Vault(
            ctoken,
            Alice,
            Alice,
            tokenIds,
            amounts,
            bytes32(0),
            address(erc1155Vault),
            srcChainId,
            0
        );

        // Query canonicalToBridged
        address deployedContract =
            destChainErc1155Vault.canonicalToBridged(srcChainId, address(ctoken1155));

        try UpdatedBridgedERC1155(deployedContract).helloWorld() {
            fail();
        } catch { }

        // Upgrade the implementation of that contract
        // so that it supports now the 'helloWorld' call
        UpdatedBridgedERC1155 newBridgedContract = new UpdatedBridgedERC1155();
        vm.prank(Carol, Carol);
        BridgedERC1155(payable(deployedContract)).upgradeTo(address(newBridgedContract));

        try UpdatedBridgedERC1155(deployedContract).helloWorld() { }
        catch {
            fail();
        }
    }

    function test_1155Vault_shall_not_be_able_to_burn_arbitrarily() public {
        vm.prank(Alice, Alice);
        ctoken1155.setApprovalForAll(address(erc1155Vault), true);

        assertEq(ctoken1155.balanceOf(Alice, 1), 10);
        assertEq(ctoken1155.balanceOf(address(erc1155Vault), 1), 0);

        uint256[] memory tokenIds = new uint256[](1);
        tokenIds[0] = 1;

        uint256[] memory amounts = new uint256[](1);
        amounts[0] = 1;

        BaseNFTVault.BridgeTransferOp memory sendOpts = BaseNFTVault.BridgeTransferOp(
            destChainId,
            address(0),
            Alice,
            GAS_LIMIT,
            address(ctoken1155),
            GAS_LIMIT,
            tokenIds,
            amounts
        );
        vm.prank(Alice, Alice);
        erc1155Vault.sendToken{ value: GAS_LIMIT }(sendOpts);

        assertEq(ctoken1155.balanceOf(address(erc1155Vault), 1), 1);

        // This canonicalToken is basically need to be exact same as the
        // sendToken() puts together
        // - here is just mocking putting it together.
        BaseNFTVault.CanonicalNFT memory canonicalToken = BaseNFTVault.CanonicalNFT({
            chainId: 31_337,
            addr: address(ctoken1155),
            symbol: "TT",
            name: "TT"
        });

        uint64 chainId = uint64(block.chainid);
        vm.chainId(destChainId);

        destChainIdBridge.sendReceiveERC1155ToERC1155Vault(
            canonicalToken,
            Alice,
            Alice,
            tokenIds,
            amounts,
            bytes32(0),
            address(erc1155Vault),
            chainId,
            0
        );

        // Query canonicalToBridged
        address deployedContract =
            destChainErc1155Vault.canonicalToBridged(chainId, address(ctoken1155));
        // Alice bridged over 1 from tokenId 1
        assertEq(ERC1155(deployedContract).balanceOf(Alice, 1), 1);

        sendOpts = BaseNFTVault.BridgeTransferOp(
            chainId,
            address(0),
            Alice,
            GAS_LIMIT,
            address(deployedContract),
            GAS_LIMIT,
            tokenIds,
            amounts
        );

        // Alice hasn't approved the vault yet!
        vm.prank(Alice, Alice);
        vm.expectRevert("ERC1155: caller is not token owner or approved");
        destChainErc1155Vault.sendToken{ value: GAS_LIMIT }(sendOpts);

        // Also Vault cannot burn tokens it does not own (even if the priv key compromised)
        vm.prank(address(destChainErc1155Vault), address(destChainErc1155Vault));
        vm.expectRevert("ERC1155: burn amount exceeds balance");
        BridgedERC1155(deployedContract).burn(1, 20);

        // After setApprovalForAll() ERC1155Vault can transfer and burn
        vm.prank(Alice, Alice);
        ERC1155(deployedContract).setApprovalForAll(address(destChainErc1155Vault), true);
        vm.prank(Alice, Alice);
        destChainErc1155Vault.sendToken{ value: GAS_LIMIT }(sendOpts);
    }
}
