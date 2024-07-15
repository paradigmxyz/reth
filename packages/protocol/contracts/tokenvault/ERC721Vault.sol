// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import "@openzeppelin/contracts/token/ERC721/IERC721.sol";
import "@openzeppelin/contracts/token/ERC721/IERC721Receiver.sol";
import "../libs/LibAddress.sol";
import "../common/LibStrings.sol";
import "./IBridgedERC721.sol";
import "./BaseNFTVault.sol";

/// @title ERC721Vault
/// @notice This vault holds all ERC721 tokens that users have deposited. It also manages
/// the mapping between canonical tokens and their bridged tokens.
/// @dev Labeled in AddressResolver as "erc721_vault".
/// @custom:security-contact security@taiko.xyz
contract ERC721Vault is BaseNFTVault, IERC721Receiver {
    using LibAddress for address;

    uint256[50] private __gap;

    /// @notice Initializes the contract.
    /// @param _owner The owner of this contract. msg.sender will be used if this value is zero.
    /// @param _addressManager The address of the {AddressManager} contract.
    function init(address _owner, address _addressManager) external initializer {
        __Essential_init(_owner, _addressManager);
    }

    /// @notice Transfers ERC721 tokens to this vault and sends a message to the
    /// destination chain so the user can receive the same (bridged) tokens
    /// by invoking the message call.
    /// @param _op Option for sending the ERC721 token.
    /// @return message_ The constructed message.
    function sendToken(BridgeTransferOp calldata _op)
        external
        payable
        whenNotPaused
        withValidOperation(_op)
        nonReentrant
        returns (IBridge.Message memory message_)
    {
        if (msg.value < _op.fee) revert VAULT_INSUFFICIENT_FEE();

        for (uint256 i; i < _op.tokenIds.length; ++i) {
            if (_op.amounts[i] != 0) revert VAULT_INVALID_AMOUNT();
        }

        if (!_op.token.supportsInterface(type(IERC721).interfaceId)) {
            revert VAULT_INTERFACE_NOT_SUPPORTED();
        }

        (bytes memory data, CanonicalNFT memory ctoken) = _handleMessage(_op);

        IBridge.Message memory message = IBridge.Message({
            id: 0, // will receive a new value
            from: address(0), // will receive a new value
            srcChainId: 0, // will receive a new value
            destChainId: _op.destChainId,
            srcOwner: msg.sender,
            destOwner: _op.destOwner != address(0) ? _op.destOwner : msg.sender,
            to: resolve(_op.destChainId, name(), false),
            value: msg.value - _op.fee,
            fee: _op.fee,
            gasLimit: _op.gasLimit,
            data: data
        });

        bytes32 msgHash;
        (msgHash, message_) =
            IBridge(resolve(LibStrings.B_BRIDGE, false)).sendMessage{ value: msg.value }(message);

        emit TokenSent({
            msgHash: msgHash,
            from: message_.srcOwner,
            to: _op.to,
            destChainId: message_.destChainId,
            ctoken: ctoken.addr,
            token: _op.token,
            tokenIds: _op.tokenIds,
            amounts: _op.amounts
        });
    }

    /// @inheritdoc IMessageInvocable
    function onMessageInvocation(bytes calldata _data)
        external
        payable
        whenNotPaused
        nonReentrant
    {
        (CanonicalNFT memory ctoken, address from, address to, uint256[] memory tokenIds) =
            abi.decode(_data, (CanonicalNFT, address, address, uint256[]));

        // `onlyFromBridge` checked in checkProcessMessageContext
        IBridge.Context memory ctx = checkProcessMessageContext();

        // Don't allow sending to disallowed addresses.
        // Don't send the tokens back to `from` because `from` is on the source chain.
        checkToAddress(to);

        // Transfer the ETH and the tokens to the `to` address
        address token = _transferTokens(ctoken, to, tokenIds);
        to.sendEtherAndVerify(msg.value);

        emit TokenReceived({
            msgHash: ctx.msgHash,
            from: from,
            to: to,
            srcChainId: ctx.srcChainId,
            ctoken: ctoken.addr,
            token: token,
            tokenIds: tokenIds,
            amounts: new uint256[](tokenIds.length)
        });
    }

    /// @inheritdoc IRecallableSender
    function onMessageRecalled(
        IBridge.Message calldata _message,
        bytes32 _msgHash
    )
        external
        payable
        override
        whenNotPaused
        nonReentrant
    {
        // `onlyFromBridge` checked in checkRecallMessageContext
        checkRecallMessageContext();

        (bytes memory data) = abi.decode(_message.data[4:], (bytes));
        (CanonicalNFT memory ctoken,,, uint256[] memory tokenIds) =
            abi.decode(data, (CanonicalNFT, address, address, uint256[]));

        // Transfer the ETH and tokens back to the owner
        address token = _transferTokens(ctoken, _message.srcOwner, tokenIds);
        _message.srcOwner.sendEtherAndVerify(_message.value);

        emit TokenReleased({
            msgHash: _msgHash,
            from: _message.srcOwner,
            ctoken: ctoken.addr,
            token: token,
            tokenIds: tokenIds,
            amounts: new uint256[](tokenIds.length)
        });
    }

    /// @inheritdoc IERC721Receiver
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
        return IERC721Receiver.onERC721Received.selector;
    }

    /// @inheritdoc BaseVault
    function name() public pure override returns (bytes32) {
        return LibStrings.B_ERC721_VAULT;
    }

    function _transferTokens(
        CanonicalNFT memory _ctoken,
        address _to,
        uint256[] memory _tokenIds
    )
        private
        returns (address token_)
    {
        if (_ctoken.chainId == block.chainid) {
            token_ = _ctoken.addr;
            for (uint256 i; i < _tokenIds.length; ++i) {
                IERC721(token_).safeTransferFrom(address(this), _to, _tokenIds[i]);
            }
        } else {
            token_ = _getOrDeployBridgedToken(_ctoken);
            for (uint256 i; i < _tokenIds.length; ++i) {
                IBridgedERC721(token_).mint(_to, _tokenIds[i]);
            }
        }
    }

    /// @dev Handles the message on the source chain and returns the encoded
    /// call on the destination call.
    /// @param _op BridgeTransferOp data.
    /// @return msgData_ Encoded message data.
    /// @return ctoken_ The canonical token.
    function _handleMessage(BridgeTransferOp calldata _op)
        private
        returns (bytes memory msgData_, CanonicalNFT memory ctoken_)
    {
        unchecked {
            CanonicalNFT storage _ctoken = bridgedToCanonical[_op.token];
            if (_ctoken.addr != address(0)) {
                ctoken_ = _ctoken;
                for (uint256 i; i < _op.tokenIds.length; ++i) {
                    IERC721(_op.token).safeTransferFrom(msg.sender, address(this), _op.tokenIds[i]);
                    IBridgedERC721(_op.token).burn(_op.tokenIds[i]);
                }
            } else {
                ctoken_ = CanonicalNFT({
                    chainId: uint64(block.chainid),
                    addr: _op.token,
                    symbol: safeSymbol(_op.token),
                    name: safeName(_op.token)
                });

                for (uint256 i; i < _op.tokenIds.length; ++i) {
                    IERC721(_op.token).safeTransferFrom(msg.sender, address(this), _op.tokenIds[i]);
                }
            }
        }

        msgData_ = abi.encodeCall(
            this.onMessageInvocation, abi.encode(ctoken_, msg.sender, _op.to, _op.tokenIds)
        );
    }

    /// @dev Retrieve or deploy a bridged ERC721 token contract.
    /// @param _ctoken CanonicalNFT data.
    /// @return btoken_ Address of the bridged token contract.
    function _getOrDeployBridgedToken(CanonicalNFT memory _ctoken)
        private
        returns (address btoken_)
    {
        btoken_ = canonicalToBridged[_ctoken.chainId][_ctoken.addr];

        if (btoken_ == address(0)) {
            btoken_ = _deployBridgedToken(_ctoken);
        }
    }

    /// @dev Deploy a new BridgedNFT contract and initialize it.
    /// This must be called before the first time a bridged token is sent to
    /// this chain.
    /// @param _ctoken CanonicalNFT data.
    /// @return btoken_ Address of the deployed bridged token contract.
    function _deployBridgedToken(CanonicalNFT memory _ctoken) private returns (address btoken_) {
        bytes memory data = abi.encodeCall(
            IBridgedERC721Initializable.init,
            (owner(), addressManager, _ctoken.addr, _ctoken.chainId, _ctoken.symbol, _ctoken.name)
        );

        btoken_ = address(new ERC1967Proxy(resolve(LibStrings.B_BRIDGED_ERC721, false), data));
        bridgedToCanonical[btoken_] = _ctoken;
        canonicalToBridged[_ctoken.chainId][_ctoken.addr] = btoken_;

        emit BridgedTokenDeployed({
            chainId: _ctoken.chainId,
            ctoken: _ctoken.addr,
            btoken: btoken_,
            ctokenSymbol: _ctoken.symbol,
            ctokenName: _ctoken.name
        });
    }
}
