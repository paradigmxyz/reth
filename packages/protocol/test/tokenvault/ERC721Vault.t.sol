// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import "@openzeppelin/contracts/token/ERC721/ERC721.sol";
import "../TaikoTest.sol";

contract TestTokenERC721 is ERC721 {
    string _baseTokenURI;
    uint256 minted;

    constructor(string memory baseURI) ERC721("TT", "TT") {
        setBaseURI(baseURI);
    }

    function setBaseURI(string memory baseURI) internal {
        _baseTokenURI = baseURI;
    }

    function _baseURI() internal view virtual override returns (string memory) {
        return _baseTokenURI;
    }

    function mint(uint256 amount) public {
        for (uint256 i; i < amount; ++i) {
            _safeMint(msg.sender, minted + i);
        }
        minted += amount;
    }
}

// PrankDestBridge lets us simulate a transaction to the erc721Vault
// from a named Bridge, without having to test/run through the real Bridge code,
// outside the scope of the unit tests in the erc721Vault.
contract PrankDestBridge {
    ERC721Vault destERC721Vault;

    struct BridgeContext {
        bytes32 msgHash;
        address sender;
        uint64 chainId;
    }

    BridgeContext ctx;

    constructor(ERC721Vault _erc721Vault) {
        destERC721Vault = _erc721Vault;
    }

    function setERC721Vault(address addr) public {
        destERC721Vault = ERC721Vault(addr);
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

    function sendReceiveERC721ToERC721Vault(
        BaseNFTVault.CanonicalNFT calldata canonicalToken,
        address from,
        address to,
        uint256[] memory tokenIds,
        bytes32 msgHash,
        address srcChainerc721Vault,
        uint64 chainId,
        uint256 mockLibInvokeMsgValue
    )
        public
    {
        ctx.sender = srcChainerc721Vault;
        ctx.msgHash = msgHash;
        ctx.chainId = chainId;

        // We need this in order to 'mock' the LibBridgeInvoke's
        //  (success,retVal) =
        //     message.to.call{ value: message.value, gas: gasLimit
        // }(message.data);
        // The problem (with foundry) is that this way it is not able to deploy
        // a contract
        // most probably due to some deployment address nonce issue. (Seems a
        // known issue).
        destERC721Vault.onMessageInvocation{ value: mockLibInvokeMsgValue }(
            abi.encode(canonicalToken, from, to, tokenIds)
        );

        ctx.sender = address(0);
        ctx.msgHash = bytes32(0);
        ctx.chainId = 0;
    }
}

contract UpdatedBridgedERC721 is BridgedERC721 {
    function helloWorld() public pure returns (string memory) {
        return "helloworld";
    }
}

contract ERC721VaultTest is TaikoTest {
    uint32 private constant GAS_LIMIT = 2_000_000;

    AddressManager addressManager;
    BadReceiver badReceiver;
    Bridge bridge;
    Bridge destChainBridge;
    PrankDestBridge destChainIdBridge;
    SkipProofCheckSignal mockProofSignalService;
    ERC721Vault erc721Vault;
    ERC721Vault destChainErc721Vault;
    TestTokenERC721 canonicalToken721;
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

        erc721Vault = ERC721Vault(
            deployProxy({
                name: "erc721_vault",
                impl: address(new ERC721Vault()),
                data: abi.encodeCall(ERC721Vault.init, (address(0), address(addressManager)))
            })
        );

        destChainErc721Vault = ERC721Vault(
            deployProxy({
                name: "erc721_vault",
                impl: address(new ERC721Vault()),
                data: abi.encodeCall(ERC721Vault.init, (address(0), address(addressManager)))
            })
        );

        destChainIdBridge = new PrankDestBridge(destChainErc721Vault);
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

        addressManager.setAddress(uint64(block.chainid), "erc721_vault", address(erc721Vault));

        addressManager.setAddress(destChainId, "erc721_vault", address(destChainErc721Vault));
        // Below 2-2 registrations (mock) are needed bc of
        // LibBridgeRecall.sol's
        // resolve address
        addressManager.setAddress(destChainId, "erc1155_vault", address(erc721Vault));
        addressManager.setAddress(destChainId, "erc20_vault", address(erc721Vault));
        addressManager.setAddress(uint64(block.chainid), "erc1155_vault", address(erc721Vault));
        addressManager.setAddress(uint64(block.chainid), "erc20_vault", address(erc721Vault));

        address bridgedERC721 = address(new BridgedERC721());

        addressManager.setAddress(destChainId, "bridged_erc721", bridgedERC721);
        addressManager.setAddress(uint64(block.chainid), "bridged_erc721", bridgedERC721);

        vm.stopPrank();

        vm.startPrank(Alice);
        canonicalToken721 = new TestTokenERC721("http://example.host.com/");
        canonicalToken721.mint(10);
        vm.stopPrank();
    }

    function getPreDeterminedDataBytes() internal pure returns (bytes memory) {
        return
        hex"a9976baf00000000000000000000000000000000000000000000000000000000000000800000000000000000000000007e5f4552091a69125d5dfcb7b8c2659029395bdf0000000000000000000000007e5f4552091a69125d5dfcb7b8c2659029395bdf00000000000000000000000000000000000000000000000000000000000001800000000000000000000000000000000000000000000000000000000000007a69000000000000000000000000f2e246bb76df876cef8b38ae84130f4f55de395b000000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000000000c0000000000000000000000000000000000000000000000000000000000000000254540000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002545400000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000000000000000000000001";
    }

    function test_721Vault_sendToken_721() public {
        vm.prank(Alice, Alice);
        canonicalToken721.approve(address(erc721Vault), 1);

        assertEq(canonicalToken721.ownerOf(1), Alice);

        uint256[] memory tokenIds = new uint256[](1);
        tokenIds[0] = 1;

        uint256[] memory amounts = new uint256[](1);
        amounts[0] = 0;

        BaseNFTVault.BridgeTransferOp memory sendOpts = BaseNFTVault.BridgeTransferOp(
            destChainId,
            address(0),
            Alice,
            GAS_LIMIT,
            address(canonicalToken721),
            GAS_LIMIT,
            tokenIds,
            amounts // With ERC721 still need to specify 1
        );
        vm.prank(Alice, Alice);
        erc721Vault.sendToken{ value: GAS_LIMIT }(sendOpts);

        assertEq(ERC721(canonicalToken721).ownerOf(1), address(erc721Vault));
    }

    function test_721Vault_sendToken_with_invalid_token_address() public {
        vm.prank(Alice, Alice);
        canonicalToken721.approve(address(erc721Vault), 1);

        assertEq(canonicalToken721.ownerOf(1), Alice);

        uint256[] memory tokenIds = new uint256[](1);
        tokenIds[0] = 1;

        uint256[] memory amounts = new uint256[](1);
        amounts[0] = 0;

        BaseNFTVault.BridgeTransferOp memory sendOpts = BaseNFTVault.BridgeTransferOp(
            destChainId, address(0), Alice, GAS_LIMIT, address(0), GAS_LIMIT, tokenIds, amounts
        );
        vm.prank(Alice, Alice);
        vm.expectRevert(BaseNFTVault.VAULT_INVALID_TOKEN.selector);
        erc721Vault.sendToken{ value: GAS_LIMIT }(sendOpts);
    }

    function test_721Vault_sendToken_with_1_tokens_but_erc721_amount_1_invalid() public {
        vm.prank(Alice, Alice);
        canonicalToken721.approve(address(erc721Vault), 1);

        assertEq(canonicalToken721.ownerOf(1), Alice);

        uint256[] memory tokenIds = new uint256[](1);
        tokenIds[0] = 1;

        uint256[] memory amounts = new uint256[](1);
        amounts[0] = 1;
        BaseNFTVault.BridgeTransferOp memory sendOpts = BaseNFTVault.BridgeTransferOp(
            destChainId,
            address(0),
            Alice,
            GAS_LIMIT,
            address(canonicalToken721),
            GAS_LIMIT,
            tokenIds,
            amounts
        );
        vm.prank(Alice, Alice);
        vm.expectRevert(BaseNFTVault.VAULT_INVALID_AMOUNT.selector);
        erc721Vault.sendToken{ value: GAS_LIMIT }(sendOpts);
    }

    function test_721Vault_receiveTokens_from_newly_deployed_bridged_contract_on_destination_chain_721(
    )
        public
    {
        vm.prank(Alice, Alice);
        canonicalToken721.approve(address(erc721Vault), 1);

        assertEq(canonicalToken721.ownerOf(1), Alice);

        uint256[] memory tokenIds = new uint256[](1);
        tokenIds[0] = 1;

        uint256[] memory amounts = new uint256[](1);
        amounts[0] = 0;

        BaseNFTVault.BridgeTransferOp memory sendOpts = BaseNFTVault.BridgeTransferOp(
            destChainId,
            address(0),
            Alice,
            GAS_LIMIT,
            address(canonicalToken721),
            GAS_LIMIT,
            tokenIds,
            amounts
        );
        vm.prank(Alice, Alice);
        erc721Vault.sendToken{ value: GAS_LIMIT }(sendOpts);

        assertEq(canonicalToken721.ownerOf(1), address(erc721Vault));

        BaseNFTVault.CanonicalNFT memory canonicalToken = BaseNFTVault.CanonicalNFT({
            chainId: 31_337,
            addr: address(canonicalToken721),
            symbol: "TT",
            name: "TT"
        });

        uint64 chainId = uint64(block.chainid);
        vm.chainId(destChainId);

        destChainIdBridge.sendReceiveERC721ToERC721Vault(
            canonicalToken, Alice, Alice, tokenIds, bytes32(0), address(erc721Vault), chainId, 0
        );

        // Query canonicalToBridged
        address deployedContract =
            destChainErc721Vault.canonicalToBridged(chainId, address(canonicalToken721));

        // Alice bridged over tokenId 1
        assertEq(ERC721(deployedContract).ownerOf(1), Alice);
    }

    function test_721Vault_receiveTokens_but_mint_not_deploy_if_bridged_second_time_721() public {
        vm.prank(Alice, Alice);
        canonicalToken721.approve(address(erc721Vault), 1);
        vm.prank(Alice, Alice);
        canonicalToken721.approve(address(erc721Vault), 2);

        assertEq(canonicalToken721.ownerOf(1), Alice);

        uint256[] memory tokenIds = new uint256[](1);
        tokenIds[0] = 1;

        uint256[] memory amounts = new uint256[](1);
        amounts[0] = 0;

        BaseNFTVault.BridgeTransferOp memory sendOpts = BaseNFTVault.BridgeTransferOp(
            destChainId,
            address(0),
            Alice,
            GAS_LIMIT,
            address(canonicalToken721),
            GAS_LIMIT,
            tokenIds,
            amounts
        );
        vm.prank(Alice, Alice);
        erc721Vault.sendToken{ value: GAS_LIMIT }(sendOpts);

        assertEq(canonicalToken721.ownerOf(1), address(erc721Vault));

        // This canonicalToken is basically need to be exact same as the
        // sendToken() puts together
        // - here is just mocking putting it together.
        BaseNFTVault.CanonicalNFT memory canonicalToken = BaseNFTVault.CanonicalNFT({
            chainId: 31_337,
            addr: address(canonicalToken721),
            symbol: "TT",
            name: "TT"
        });

        uint64 chainId = uint64(block.chainid);
        vm.chainId(destChainId);

        destChainIdBridge.sendReceiveERC721ToERC721Vault(
            canonicalToken, Alice, Alice, tokenIds, bytes32(0), address(erc721Vault), chainId, 0
        );

        // Query canonicalToBridged
        address deployedContract =
            destChainErc721Vault.canonicalToBridged(chainId, address(canonicalToken721));

        // Alice bridged over tokenId 1
        assertEq(ERC721(deployedContract).ownerOf(1), Alice);

        // Change back to 'L1'
        vm.chainId(chainId);

        tokenIds[0] = 2;

        amounts[0] = 0;

        sendOpts = BaseNFTVault.BridgeTransferOp(
            destChainId,
            address(0),
            Alice,
            GAS_LIMIT,
            address(canonicalToken721),
            GAS_LIMIT,
            tokenIds,
            amounts
        );
        vm.prank(Alice, Alice);
        erc721Vault.sendToken{ value: GAS_LIMIT }(sendOpts);

        assertEq(canonicalToken721.ownerOf(2), address(erc721Vault));

        vm.chainId(destChainId);

        destChainIdBridge.sendReceiveERC721ToERC721Vault(
            canonicalToken, Alice, Alice, tokenIds, bytes32(0), address(erc721Vault), chainId, 0
        );

        // Query canonicalToBridged
        address bridgedContract =
            destChainErc721Vault.canonicalToBridged(chainId, address(canonicalToken721));

        assertEq(bridgedContract, deployedContract);
    }

    function test_721Vault_receiveTokens_erc721_with_ether_to_dave() public {
        vm.prank(Alice, Alice);
        canonicalToken721.approve(address(erc721Vault), 1);

        assertEq(canonicalToken721.ownerOf(1), Alice);

        uint256[] memory tokenIds = new uint256[](1);
        tokenIds[0] = 1;

        uint256[] memory amounts = new uint256[](1);
        amounts[0] = 0;

        uint256 etherValue = 0.1 ether;
        BaseNFTVault.BridgeTransferOp memory sendOpts = BaseNFTVault.BridgeTransferOp(
            destChainId,
            address(0),
            David,
            GAS_LIMIT,
            address(canonicalToken721),
            GAS_LIMIT,
            tokenIds,
            amounts
        );
        vm.prank(Alice, Alice);
        erc721Vault.sendToken{ value: etherValue }(sendOpts);

        assertEq(canonicalToken721.ownerOf(1), address(erc721Vault));

        BaseNFTVault.CanonicalNFT memory canonicalToken = BaseNFTVault.CanonicalNFT({
            chainId: 31_337,
            addr: address(canonicalToken721),
            symbol: "TT",
            name: "TT"
        });

        uint64 chainId = uint64(block.chainid);
        vm.chainId(destChainId);

        destChainIdBridge.sendReceiveERC721ToERC721Vault(
            canonicalToken,
            Alice,
            David,
            tokenIds,
            bytes32(0),
            address(erc721Vault),
            chainId,
            etherValue
        );

        // Query canonicalToBridged
        address deployedContract =
            destChainErc721Vault.canonicalToBridged(chainId, address(canonicalToken721));

        // Alice bridged over tokenId 1 and etherValue to David
        assertEq(ERC721(deployedContract).ownerOf(1), David);
        assertEq(etherValue, David.balance);
    }

    function test_721Vault_onMessageRecalled_721() public {
        vm.prank(Alice, Alice);
        canonicalToken721.approve(address(erc721Vault), 1);

        assertEq(canonicalToken721.ownerOf(1), Alice);

        uint256[] memory tokenIds = new uint256[](1);
        tokenIds[0] = 1;

        uint256[] memory amounts = new uint256[](1);
        amounts[0] = 0;

        BaseNFTVault.BridgeTransferOp memory sendOpts = BaseNFTVault.BridgeTransferOp(
            destChainId,
            address(0),
            Alice,
            GAS_LIMIT,
            address(canonicalToken721),
            GAS_LIMIT,
            tokenIds,
            amounts
        );

        vm.prank(Alice, Alice);
        IBridge.Message memory message = erc721Vault.sendToken{ value: GAS_LIMIT }(sendOpts);

        assertEq(canonicalToken721.ownerOf(1), address(erc721Vault));

        bridge.recallMessage(message, bytes(""));

        // Alice got back her NFT
        assertEq(canonicalToken721.ownerOf(1), Alice);
    }

    function test_721Vault_receiveTokens_multiple_721() public {
        vm.prank(Alice, Alice);
        canonicalToken721.approve(address(erc721Vault), 1);
        vm.prank(Alice, Alice);
        canonicalToken721.approve(address(erc721Vault), 2);

        assertEq(canonicalToken721.ownerOf(1), Alice);
        assertEq(canonicalToken721.ownerOf(2), Alice);

        uint256[] memory tokenIds = new uint256[](2);
        tokenIds[0] = 1;
        tokenIds[1] = 2;

        uint256[] memory amounts = new uint256[](2);
        amounts[0] = 0;
        amounts[1] = 0;

        BaseNFTVault.BridgeTransferOp memory sendOpts = BaseNFTVault.BridgeTransferOp(
            destChainId,
            address(0),
            Alice,
            GAS_LIMIT,
            address(canonicalToken721),
            GAS_LIMIT,
            tokenIds,
            amounts
        );
        vm.prank(Alice, Alice);
        erc721Vault.sendToken{ value: GAS_LIMIT }(sendOpts);

        assertEq(canonicalToken721.ownerOf(1), address(erc721Vault));
        assertEq(canonicalToken721.ownerOf(2), address(erc721Vault));

        BaseNFTVault.CanonicalNFT memory canonicalToken = BaseNFTVault.CanonicalNFT({
            chainId: 31_337,
            addr: address(canonicalToken721),
            symbol: "TT",
            name: "TT"
        });

        uint64 srcChainId = uint64(block.chainid);
        vm.chainId(destChainId);

        destChainIdBridge.sendReceiveERC721ToERC721Vault(
            canonicalToken, Alice, Alice, tokenIds, bytes32(0), address(erc721Vault), srcChainId, 0
        );

        // Query canonicalToBridged
        address deployedContract =
            destChainErc721Vault.canonicalToBridged(srcChainId, address(canonicalToken721));

        // Alice bridged over tokenId 1
        assertEq(ERC721(deployedContract).ownerOf(1), Alice);
        assertEq(ERC721(deployedContract).ownerOf(2), Alice);
    }

    function test_721Vault_bridge_back_but_owner_is_different_now_721() public {
        vm.prank(Alice, Alice);
        canonicalToken721.approve(address(erc721Vault), 1);
        vm.prank(Alice, Alice);
        canonicalToken721.approve(address(erc721Vault), 2);

        assertEq(canonicalToken721.ownerOf(1), Alice);

        uint256[] memory tokenIds = new uint256[](1);
        tokenIds[0] = 1;

        uint256[] memory amounts = new uint256[](1);
        amounts[0] = 0;

        BaseNFTVault.BridgeTransferOp memory sendOpts = BaseNFTVault.BridgeTransferOp(
            destChainId,
            address(0),
            Alice,
            GAS_LIMIT,
            address(canonicalToken721),
            GAS_LIMIT,
            tokenIds,
            amounts
        );
        vm.prank(Alice, Alice);
        erc721Vault.sendToken{ value: GAS_LIMIT }(sendOpts);

        assertEq(canonicalToken721.ownerOf(1), address(erc721Vault));

        // This canonicalToken is basically need to be exact same as the
        // sendToken() puts together
        // - here is just mocking putting it together.
        BaseNFTVault.CanonicalNFT memory canonicalToken = BaseNFTVault.CanonicalNFT({
            chainId: 31_337,
            addr: address(canonicalToken721),
            symbol: "TT",
            name: "TT"
        });

        uint64 chainId = uint64(block.chainid);
        vm.chainId(destChainId);

        destChainIdBridge.sendReceiveERC721ToERC721Vault(
            canonicalToken, Alice, Alice, tokenIds, bytes32(0), address(erc721Vault), chainId, 0
        );

        // Query canonicalToBridged
        address deployedContract =
            destChainErc721Vault.canonicalToBridged(chainId, address(canonicalToken721));

        // Alice bridged over tokenId 1
        assertEq(ERC721(deployedContract).ownerOf(1), Alice);

        // Transfer the asset to Bob, and Bob can receive it back on canonical
        // chain
        vm.prank(Alice, Alice);
        ERC721(deployedContract).transferFrom(Alice, Bob, 1);

        assertEq(ERC721(deployedContract).ownerOf(1), Bob);

        vm.prank(Bob, Bob);
        ERC721(deployedContract).approve(address(destChainErc721Vault), 1);

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
        destChainErc721Vault.sendToken{ value: GAS_LIMIT }(sendOpts);

        vm.chainId(chainId);

        assertEq(ERC721(canonicalToken721).ownerOf(1), address(erc721Vault));

        destChainIdBridge.setERC721Vault(address(erc721Vault));

        vm.prank(Carol, Carol);
        addressManager.setAddress(uint64(block.chainid), "bridge", address(destChainIdBridge));

        destChainIdBridge.sendReceiveERC721ToERC721Vault(
            canonicalToken, Bob, Bob, tokenIds, bytes32(0), address(erc721Vault), chainId, 0
        );

        assertEq(canonicalToken721.ownerOf(1), Bob);
    }

    function test_721Vault_bridge_back_but_original_owner_cannot_claim_it_anymore_if_sold_721()
        public
    {
        vm.prank(Alice, Alice);
        canonicalToken721.approve(address(erc721Vault), 1);
        vm.prank(Alice, Alice);
        canonicalToken721.approve(address(erc721Vault), 2);

        assertEq(canonicalToken721.ownerOf(1), Alice);

        uint256[] memory tokenIds = new uint256[](1);
        tokenIds[0] = 1;

        uint256[] memory amounts = new uint256[](1);
        amounts[0] = 0;

        BaseNFTVault.BridgeTransferOp memory sendOpts = BaseNFTVault.BridgeTransferOp(
            destChainId,
            address(0),
            Alice,
            GAS_LIMIT,
            address(canonicalToken721),
            GAS_LIMIT,
            tokenIds,
            amounts
        );
        vm.prank(Alice, Alice);
        erc721Vault.sendToken{ value: GAS_LIMIT }(sendOpts);

        assertEq(canonicalToken721.ownerOf(1), address(erc721Vault));

        // This canonicalToken is basically need to be exact same as the
        // sendToken() puts together
        // - here is just mocking putting it together.
        BaseNFTVault.CanonicalNFT memory canonicalToken = BaseNFTVault.CanonicalNFT({
            chainId: 31_337,
            addr: address(canonicalToken721),
            symbol: "TT",
            name: "TT"
        });

        uint64 chainId = uint64(block.chainid);
        vm.chainId(destChainId);

        destChainIdBridge.sendReceiveERC721ToERC721Vault(
            canonicalToken, Alice, Alice, tokenIds, bytes32(0), address(erc721Vault), chainId, 0
        );

        // Query canonicalToBridged
        address deployedContract =
            destChainErc721Vault.canonicalToBridged(chainId, address(canonicalToken721));

        // Alice bridged over tokenId 1
        assertEq(ERC721(deployedContract).ownerOf(1), Alice);

        // Transfer the asset to Bob, and Bob can receive it back on canonical
        // chain
        vm.prank(Alice, Alice);
        ERC721(deployedContract).transferFrom(Alice, Bob, 1);

        assertEq(ERC721(deployedContract).ownerOf(1), Bob);

        vm.prank(Bob, Bob);
        ERC721(deployedContract).approve(address(destChainErc721Vault), 1);

        // Alice puts together a malicious bridging back message
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
        vm.expectRevert("ERC721: transfer from incorrect owner");
        destChainErc721Vault.sendToken{ value: GAS_LIMIT }(sendOpts);
    }

    function test_721Vault_upgrade_bridged_tokens_721() public {
        vm.prank(Alice, Alice);
        canonicalToken721.approve(address(erc721Vault), 1);
        vm.prank(Alice, Alice);
        canonicalToken721.approve(address(erc721Vault), 2);

        assertEq(canonicalToken721.ownerOf(1), Alice);

        uint256[] memory tokenIds = new uint256[](1);
        tokenIds[0] = 1;

        uint256[] memory amounts = new uint256[](1);
        amounts[0] = 0;

        BaseNFTVault.BridgeTransferOp memory sendOpts = BaseNFTVault.BridgeTransferOp(
            destChainId,
            address(0),
            Alice,
            GAS_LIMIT,
            address(canonicalToken721),
            GAS_LIMIT,
            tokenIds,
            amounts
        );
        vm.prank(Alice, Alice);
        erc721Vault.sendToken{ value: GAS_LIMIT }(sendOpts);

        assertEq(canonicalToken721.ownerOf(1), address(erc721Vault));

        // This canonicalToken is basically need to be exact same as the
        // sendToken() puts together
        // - here is just mocking putting it together.
        BaseNFTVault.CanonicalNFT memory canonicalToken = BaseNFTVault.CanonicalNFT({
            chainId: 31_337,
            addr: address(canonicalToken721),
            symbol: "TT",
            name: "TT"
        });

        uint64 chainId = uint64(block.chainid);
        vm.chainId(destChainId);

        destChainIdBridge.sendReceiveERC721ToERC721Vault(
            canonicalToken, Alice, Alice, tokenIds, bytes32(0), address(erc721Vault), chainId, 0
        );

        // Query canonicalToBridged
        address deployedContract =
            destChainErc721Vault.canonicalToBridged(chainId, address(canonicalToken721));

        try UpdatedBridgedERC721(deployedContract).helloWorld() {
            fail();
        } catch {
            // It should not yet support this function call
        }

        // Upgrade the implementation of that contract
        // so that it supports now the 'helloWorld' call
        UpdatedBridgedERC721 newBridgedContract = new UpdatedBridgedERC721();
        vm.prank(Carol, Carol);
        BridgedERC721(payable(deployedContract)).upgradeTo(address(newBridgedContract));

        try UpdatedBridgedERC721(deployedContract).helloWorld() {
            // It should support now this function call
        } catch {
            fail();
        }
    }

    function test_721Vault_shall_not_be_able_to_burn_arbitrarily() public {
        vm.prank(Alice, Alice);
        canonicalToken721.approve(address(erc721Vault), 1);
        vm.prank(Alice, Alice);
        canonicalToken721.approve(address(erc721Vault), 2);

        assertEq(canonicalToken721.ownerOf(1), Alice);

        uint256[] memory tokenIds = new uint256[](1);
        tokenIds[0] = 1;

        uint256[] memory amounts = new uint256[](1);
        amounts[0] = 0;

        BaseNFTVault.BridgeTransferOp memory sendOpts = BaseNFTVault.BridgeTransferOp(
            destChainId,
            address(0),
            Alice,
            GAS_LIMIT,
            address(canonicalToken721),
            GAS_LIMIT,
            tokenIds,
            amounts
        );
        vm.prank(Alice, Alice);
        erc721Vault.sendToken{ value: GAS_LIMIT }(sendOpts);

        assertEq(canonicalToken721.ownerOf(1), address(erc721Vault));

        // This canonicalToken is basically need to be exact same as the
        // sendToken() puts together
        // - here is just mocking putting it together.
        BaseNFTVault.CanonicalNFT memory canonicalToken = BaseNFTVault.CanonicalNFT({
            chainId: 31_337,
            addr: address(canonicalToken721),
            symbol: "TT",
            name: "TT"
        });

        uint64 chainId = uint64(block.chainid);
        vm.chainId(destChainId);

        destChainIdBridge.sendReceiveERC721ToERC721Vault(
            canonicalToken, Alice, Alice, tokenIds, bytes32(0), address(erc721Vault), chainId, 0
        );

        // Query canonicalToBridged
        address deployedContract =
            destChainErc721Vault.canonicalToBridged(chainId, address(canonicalToken721));

        // Alice bridged over tokenId 1
        assertEq(ERC721(deployedContract).ownerOf(1), Alice);

        // Alice tries to bridge back message
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
        vm.expectRevert("ERC721: caller is not token owner or approved");
        destChainErc721Vault.sendToken{ value: GAS_LIMIT }(sendOpts);

        // Also Vault cannot burn tokens it does not own (even if the priv key compromised)
        vm.prank(address(destChainErc721Vault), address(destChainErc721Vault));
        vm.expectRevert(BridgedERC721.BTOKEN_INVALID_BURN.selector);
        BridgedERC721(deployedContract).burn(1);

        // After approve() ERC721Vault can transfer and burn
        vm.prank(Alice, Alice);
        ERC721(deployedContract).approve(address(destChainErc721Vault), 1);
        vm.prank(Alice, Alice);
        destChainErc721Vault.sendToken{ value: GAS_LIMIT }(sendOpts);
    }
}
