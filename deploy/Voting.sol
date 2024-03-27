// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

contract VotingV1 {
    uint8 constant VALIDATORS_MAX = 5;
    PeerKey[] validators; // slot 0
    struct PeerKey {
        bytes32 Half1;
        bytes32 Half2;
    }
    constructor() {
        validators.push(
            PeerKey({
                Half1: 0x23fc99dc5a9411b1f74425cf82d38393f9f0dfa63360848886514eb64f8d61c9,
                Half2: 0x9a47b52df97afbad20bdbd781086a9e9e228a4d61177d85e28f8cdf5c6ae7738
            })
        );
        validators.push(
            PeerKey({
                Half1: 0x23fc99dc5a9411b1f74425cf82d38393f9f0dfa63360848886514eb64f8d61c9,
                Half2: 0x9a47b52df97afbad20bdbd781086a9e9e228a4d61177d85e28f8cdf5c6ae7738
            })
        );
    }

    function getValidators() public view returns (bytes32[] memory) {
        uint arrayLength = validators.length;
        bytes32[] memory ret = new bytes32[](arrayLength * 2);
        for (uint i = 0; i < arrayLength; i++) {
            ret[i * 2] = validators[i].Half1;
            ret[i * 2 + 1] = validators[i].Half2;
        }
        return ret;
    }
}
