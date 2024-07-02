// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

import "lib/openzeppelin-contracts/contracts/token/ERC721/IERC721.sol";
import
    "lib/openzeppelin-contracts-upgradeable/contracts/token/ERC721/IERC721ReceiverUpgradeable.sol";
import "../bridge/IBridge.sol";
import "./BaseNFTVault.sol";
import "./BridgedERC721.sol";

/// @title ERC721Vault
/// @dev Labeled in AddressResolver as "erc721_vault"
/// @notice This vault holds all ERC721 tokens that users have deposited.
/// It also manages the mapping between canonical tokens and their bridged
/// tokens.
contract ERC721Vault is BaseNFTVault, IERC721ReceiverUpgradeable {
    using LibAddress for address;

    uint256[50] private __gap;

    /// @notice Transfers ERC721 tokens to this vault and sends a message to the
    /// destination chain so the user can receive the same (bridged) tokens
    /// by invoking the message call.
    /// @param op Option for sending the ERC721 token.
    function sendToken(BridgeTransferOp calldata op)
        external
        payable
        nonReentrant
        whenNotPaused
        withValidOperation(op)
        returns (IBridge.Message memory _message)
    {
        for (uint256 i; i < op.tokenIds.length; ++i) {
            if (op.amounts[i] != 0) revert VAULT_INVALID_AMOUNT();
        }

        if (!op.token.supportsInterface(ERC721_INTERFACE_ID)) {
            revert VAULT_INTERFACE_NOT_SUPPORTED();
        }

        // We need to save them into memory - because structs containing
        // dynamic arrays will cause stack-too-deep error when passed
        uint256[] memory _amounts = op.amounts;
        address _token = op.token;
        uint256[] memory _tokenIds = op.tokenIds;

        IBridge.Message memory message;
        message.destChainId = op.destChainId;
        message.data = _handleMessage(msg.sender, op);
        message.owner = msg.sender;
        message.to = resolve(message.destChainId, name(), false);
        message.gasLimit = op.gasLimit;
        message.value = msg.value - op.fee;
        message.fee = op.fee;
        message.refundTo = op.refundTo;
        message.memo = op.memo;

        bytes32 msgHash;
        (msgHash, _message) =
            IBridge(resolve("bridge", false)).sendMessage{ value: msg.value }(message);

        emit TokenSent({
            msgHash: msgHash,
            from: _message.owner,
            to: op.to,
            destChainId: _message.destChainId,
            token: _token,
            tokenIds: _tokenIds,
            amounts: _amounts
        });
    }

    /// @notice Receive bridged ERC721 tokens and handle them accordingly.
    /// @param ctoken Canonical NFT data for the token being received.
    /// @param from Source address.
    /// @param to Destination address.
    /// @param tokenIds Array of token IDs being received.
    function receiveToken(
        CanonicalNFT calldata ctoken,
        address from,
        address to,
        uint256[] memory tokenIds
    )
        external
        payable
        nonReentrant
        whenNotPaused
    {
        IBridge.Context memory ctx = checkProcessMessageContext();

        address _to = to == address(0) || to == address(this) ? from : to;
        address token;

        unchecked {
            if (ctoken.chainId == block.chainid) {
                token = ctoken.addr;
                for (uint256 i; i < tokenIds.length; ++i) {
                    ERC721Upgradeable(token).transferFrom({
                        from: address(this),
                        to: _to,
                        tokenId: tokenIds[i]
                    });
                }
            } else {
                token = _getOrDeployBridgedToken(ctoken);
                for (uint256 i; i < tokenIds.length; ++i) {
                    BridgedERC721(token).mint(_to, tokenIds[i]);
                }
            }
        }

        _to.sendEther(msg.value);

        emit TokenReceived({
            msgHash: ctx.msgHash,
            from: from,
            to: to,
            srcChainId: ctx.srcChainId,
            token: token,
            tokenIds: tokenIds,
            amounts: new uint256[](0)
        });
    }

    function onMessageRecalled(
        IBridge.Message calldata message,
        bytes32 msgHash
    )
        external
        payable
        override
        nonReentrant
        whenNotPaused
    {
        checkRecallMessageContext();

        if (message.owner == address(0)) revert VAULT_INVALID_USER();
        if (message.srcChainId != block.chainid) {
            revert VAULT_INVALID_SRC_CHAIN_ID();
        }

        (CanonicalNFT memory nft,,, uint256[] memory tokenIds) =
            abi.decode(message.data[4:], (CanonicalNFT, address, address, uint256[]));

        if (nft.addr == address(0)) revert VAULT_INVALID_TOKEN();

        unchecked {
            if (bridgedToCanonical[nft.addr].addr != address(0)) {
                for (uint256 i; i < tokenIds.length; ++i) {
                    BridgedERC721(nft.addr).mint(message.owner, tokenIds[i]);
                }
            } else {
                for (uint256 i; i < tokenIds.length; ++i) {
                    ERC721Upgradeable(nft.addr).safeTransferFrom({
                        from: address(this),
                        to: message.owner,
                        tokenId: tokenIds[i]
                    });
                }
            }
        }

        // send back Ether
        message.owner.sendEther(message.value);

        emit TokenReleased({
            msgHash: msgHash,
            from: message.owner,
            token: nft.addr,
            tokenIds: tokenIds,
            amounts: new uint256[](0)
        });
    }

    /// @inheritdoc IERC721ReceiverUpgradeable
    function onERC721Received(
        address,
        address,
        uint256,
        bytes calldata
    )
        external
        pure
        returns (bytes4)
    {
        return IERC721ReceiverUpgradeable.onERC721Received.selector;
    }

    function name() public pure override returns (bytes32) {
        return "erc721_vault";
    }

    /// @dev Handles the message on the source chain and returns the encoded
    /// call on the destination call.
    /// @param user The user's address.
    /// @param op BridgeTransferOp data.
    /// @return msgData Encoded message data.
    function _handleMessage(
        address user,
        BridgeTransferOp calldata op
    )
        private
        returns (bytes memory msgData)
    {
        CanonicalNFT memory nft;

        unchecked {
            if (bridgedToCanonical[op.token].addr != address(0)) {
                nft = bridgedToCanonical[op.token];
                for (uint256 i; i < op.tokenIds.length; ++i) {
                    BridgedERC721(op.token).burn(user, op.tokenIds[i]);
                }
            } else {
                ERC721Upgradeable t = ERC721Upgradeable(op.token);

                nft = CanonicalNFT({
                    chainId: uint64(block.chainid),
                    addr: op.token,
                    symbol: t.symbol(),
                    name: t.name()
                });

                for (uint256 i; i < op.tokenIds.length; ++i) {
                    t.transferFrom(user, address(this), op.tokenIds[i]);
                }
            }
        }

        msgData = abi.encodeWithSelector(this.receiveToken.selector, nft, user, op.to, op.tokenIds);
    }

    /// @dev Retrieve or deploy a bridged ERC721 token contract.
    /// @param ctoken CanonicalNFT data.
    /// @return btoken Address of the bridged token contract.
    function _getOrDeployBridgedToken(CanonicalNFT calldata ctoken)
        private
        returns (address btoken)
    {
        btoken = canonicalToBridged[ctoken.chainId][ctoken.addr];

        if (btoken == address(0)) {
            btoken = _deployBridgedToken(ctoken);
        }
    }

    /// @dev Deploy a new BridgedNFT contract and initialize it.
    /// This must be called before the first time a bridged token is sent to
    /// this chain.
    /// @param ctoken CanonicalNFT data.
    /// @return btoken Address of the deployed bridged token contract.
    function _deployBridgedToken(CanonicalNFT memory ctoken) private returns (address btoken) {
        bytes memory data = bytes.concat(
            BridgedERC721.init.selector,
            abi.encode(addressManager, ctoken.addr, ctoken.chainId, ctoken.symbol, ctoken.name)
        );

        btoken = LibDeploy.deployERC1967Proxy(resolve("bridged_erc721", false), owner(), data);

        bridgedToCanonical[btoken] = ctoken;
        canonicalToBridged[ctoken.chainId][ctoken.addr] = btoken;

        emit BridgedTokenDeployed({
            chainId: ctoken.chainId,
            ctoken: ctoken.addr,
            btoken: btoken,
            ctokenSymbol: ctoken.symbol,
            ctokenName: ctoken.name
        });
    }
}
