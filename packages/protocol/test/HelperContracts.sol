// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import "../contracts/bridge/Bridge.sol";
import "../contracts/signal/SignalService.sol";

contract BadReceiver {
    receive() external payable {
        revert("can not send to this contract");
    }

    fallback() external payable {
        revert("can not send to this contract");
    }

    function transfer() public pure {
        revert("this fails");
    }
}

contract GoodReceiver is IMessageInvocable {
    receive() external payable { }

    function onMessageInvocation(bytes calldata data) public payable {
        address addr = abi.decode(data, (address));
        payable(addr).transfer(address(this).balance / 2);
    }
}

// NonNftContract
contract NonNftContract {
    uint256 dummyData;

    constructor(uint256 _dummyData) {
        dummyData = _dummyData;
    }
}

contract SkipProofCheckSignal is SignalService {
    function proveSignalReceived(
        uint64, /*srcChainId*/
        address, /*app*/
        bytes32, /*signal*/
        bytes calldata /*proof*/
    )
        public
        pure
        override
        returns (uint256)
    { }
}
