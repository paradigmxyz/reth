// SPDX-License-Identifier: MIT
//  _____     _ _         _         _
// |_   _|_ _(_) |_____  | |   __ _| |__ ___
//   | |/ _` | | / / _ \ | |__/ _` | '_ (_-<
//   |_|\__,_|_|_\_\___/ |____\__,_|_.__/__/

pragma solidity ^0.8.20;

import "../common/AddressResolver.sol";
import "../common/EssentialContract.sol";

/// @title VerifierRegistry
/// @notice A registry for handling all known verifiers
contract VerifierRegistry is EssentialContract {
    struct Verifier {
        uint16 id;
        bytes4 tag;
        bool poisoned;
    }

    mapping(address verifier => Verifier) public verifiers;
    mapping(address verifier => uint256 id) public verifierId;
    mapping(uint256 id => address verifier) public verifierAddress;

    uint16 public verifierIdGenerator;

    function init(address _owner, address _addressManager) external initializer {
        __Essential_init(_owner, _addressManager);
        verifierIdGenerator = 1;
    }

    /// Adds a verifier
    function addVerifier(address verifier, bytes4 tag) external onlyOwner {
        // Generate a unique id
        uint16 id = verifierIdGenerator++;
        verifiers[verifier] = Verifier({ id: id, tag: tag, poisoned: false });
        verifierId[verifier] = id;
        verifierAddress[id] = verifier;
    }

    /// Makes a verifier unusable
    function poisonVerifier(address verifier) external onlyFromOwnerOrNamed("verifier_watchdog") {
        delete verifiers[verifier];
    }

    function isVerifier(address addr) external view returns (bool) {
        return verifiers[addr].id != 0 && !verifiers[addr].poisoned;
    }
}
