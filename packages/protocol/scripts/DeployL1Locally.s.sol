// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import "@openzeppelin/contracts/utils/Strings.sol";

import "../contracts/L1/TaikoL1.sol";
import "../contracts/L1/BasedOperator.sol";
import "../contracts/L1/VerifierRegistry.sol";
import "../contracts/tko/TaikoToken.sol";
import "../contracts/L1/provers/GuardianProver.sol";
// import "../contracts/L1/tiers/DevnetTierProvider.sol";
// import "../contracts/L1/tiers/TierProviderV2.sol";
// import "../contracts/bridge/Bridge.sol";
// import "../contracts/tokenvault/BridgedERC20.sol";
// import "../contracts/tokenvault/BridgedERC721.sol";
// import "../contracts/tokenvault/BridgedERC1155.sol";
// import "../contracts/tokenvault/ERC20Vault.sol";
// import "../contracts/tokenvault/ERC1155Vault.sol";
// import "../contracts/tokenvault/ERC721Vault.sol";
// import "../contracts/signal/SignalService.sol";
// import "../contracts/automata-attestation/AutomataDcapV3Attestation.sol";
// import "../contracts/automata-attestation/utils/SigVerifyLib.sol";
// import "../contracts/automata-attestation/lib/PEMCertChainLib.sol";
//import "../contracts/L1/verifiers/SgxVerifier.sol";
import "../contracts/L1/verifiers/MockSgxVerifier.sol"; // Avoid proof verification for now!
// import "../contracts/team/proving/ProverSet.sol";
// import "../test/common/erc20/FreeMintERC20.sol";
// import "../test/common/erc20/MayFailFreeMintERC20.sol";
// import "../test/L1/TestTierProvider.sol";
import "../test/DeployCapability.sol";

// Actually this one is deployed already on mainnet, but we are now deploying our own (non via-ir)
// version. For mainnet, it is easier to go with one of:
// - https://github.com/daimo-eth/p256-verifier
// - https://github.com/rdubois-crypto/FreshCryptoLib
import { P256Verifier } from "p256-verifier/src/P256Verifier.sol";

/// @title DeployOnL1
/// @notice This script deploys the core Taiko protocol smart contract on L1,
/// initializing the rollup.
contract DeployL1OnAnvil is DeployCapability {
    // uint256 public NUM_MIN_MAJORITY_GUARDIANS = vm.envUint("NUM_MIN_MAJORITY_GUARDIANS");
    // uint256 public NUM_MIN_MINORITY_GUARDIANS = vm.envUint("NUM_MIN_MINORITY_GUARDIANS");

    address public MAINNET_CONTRACT_OWNER = vm.envAddress("MAINNET_CONTRACT_OWNER"); //Dani: Use an address anvil provides, with preminted ETH

    modifier broadcast() {
        uint256 privateKey = vm.envUint("PRIVATE_KEY");
        require(privateKey != 0, "invalid priv key");
        vm.startBroadcast();
        _;
        vm.stopBroadcast();
    }

    function run() external broadcast {
        /*
        IMPORTANT NOTICES:
        - TaikoL2 deployments (and not only TaikoL2, but all contracts sitting on L2) obviously not done and haven't even dealt with
        - SignalService, Bridge, Vaults also not dealt with on L1
         */
        // addressNotNull(vm.envAddress("TAIKO_L2_ADDRESS"), "TAIKO_L2_ADDRESS");
        // addressNotNull(vm.envAddress("L2_SIGNAL_SERVICE"), "L2_SIGNAL_SERVICE");
        // addressNotNull(vm.envAddress("CONTRACT_OWNER"), "CONTRACT_OWNER");

        require(vm.envBytes32("L2_GENESIS_HASH") != 0, "L2_GENESIS_HASH");
        address contractOwner = MAINNET_CONTRACT_OWNER;

        // ---------------------------------------------------------------
        // Deploy shared contracts
        (address sharedAddressManager) = deploySharedContracts(contractOwner);
        console2.log("sharedAddressManager: ", sharedAddressManager);
        // ---------------------------------------------------------------
        // Deploy rollup contracts
        address rollupAddressManager = deployRollupContracts(sharedAddressManager, contractOwner);

    //     // ---------------------------------------------------------------
    //     // Signal service need to authorize the new rollup
    //     address signalServiceAddr = AddressManager(sharedAddressManager).getAddress(
    //         uint64(block.chainid), LibStrings.B_SIGNAL_SERVICE
    //     );
    //     addressNotNull(signalServiceAddr, "signalServiceAddr");
    //     SignalService signalService = SignalService(signalServiceAddr);

        address taikoL1Addr = AddressManager(rollupAddressManager).getAddress(
            uint64(block.chainid), "taiko"
        );
        addressNotNull(taikoL1Addr, "taikoL1Addr");
        TaikoL1 taikoL1 = TaikoL1(payable(taikoL1Addr));

        // if (vm.envAddress("SHARED_ADDRESS_MANAGER") == address(0)) {
        //     SignalService(signalServiceAddr).authorize(taikoL1Addr, true);
        // }

    //     uint64 l2ChainId = taikoL1.getConfig().chainId;
    //     require(l2ChainId != block.chainid, "same chainid");

    //     console2.log("------------------------------------------");
    //     console2.log("msg.sender: ", msg.sender);
    //     console2.log("address(this): ", address(this));
    //     console2.log("signalService.owner(): ", signalService.owner());
    //     console2.log("------------------------------------------");

    //     if (signalService.owner() == msg.sender) {
    //         signalService.transferOwnership(contractOwner);
    //     } else {
    //         console2.log("------------------------------------------");
    //         console2.log("Warning - you need to transact manually:");
    //         console2.log("signalService.authorize(taikoL1Addr, bytes32(block.chainid))");
    //         console2.log("- signalService : ", signalServiceAddr);
    //         console2.log("- taikoL1Addr   : ", taikoL1Addr);
    //         console2.log("- chainId       : ", block.chainid);
    //     }

    //     // ---------------------------------------------------------------
    //     // Register L2 addresses
    //     register(rollupAddressManager, "taiko", vm.envAddress("TAIKO_L2_ADDRESS"), l2ChainId);
    //     register(
    //         rollupAddressManager, "signal_service", vm.envAddress("L2_SIGNAL_SERVICE"), l2ChainId
    //     );

    //     // ---------------------------------------------------------------
    //     // Deploy other contracts
    //     if (block.chainid != 1) {
    //         deployAuxContracts();
    //     }

    //     if (AddressManager(sharedAddressManager).owner() == msg.sender) {
    //         AddressManager(sharedAddressManager).transferOwnership(contractOwner);
    //         console2.log("** sharedAddressManager ownership transferred to:", contractOwner);
    //     }

    //     AddressManager(rollupAddressManager).transferOwnership(contractOwner);
    //     console2.log("** rollupAddressManager ownership transferred to:", contractOwner);
    }

    function deploySharedContracts(address owner) internal returns (address sharedAddressManager) {
        addressNotNull(owner, "owner");

        sharedAddressManager = address(0);// Dani: Can be set tho via ENV var, for now, for anvil, easy to just deploy every time
        if (sharedAddressManager == address(0)) {
            sharedAddressManager = deployProxy({
                name: "shared_address_manager",
                impl: address(new AddressManager()),
                data: abi.encodeCall(AddressManager.init, (owner))
            });
        }

        //dataToFeed = abi.encodeCall(TaikoToken.init, ("TAIKO", "TAIKO", MAINNET_CONTRACT_OWNER));
        address taikoToken = address(0); // Later on use this as env. var since already deployed (on testnets): vm.envAddress("TAIKO_TOKEN");
        if (taikoToken == address(0)) {
            taikoToken = deployProxy({
                name: "taiko_token",
                impl: address(new TaikoToken()),
                data: abi.encodeCall(TaikoToken.init, (MAINNET_CONTRACT_OWNER, MAINNET_CONTRACT_OWNER)),
                registerTo: sharedAddressManager
            });
        }

        // // Deploy Bridging contracts - to be done later.
        // deployProxy({
        //     name: "signal_service",
        //     impl: address(new SignalService()),
        //     data: abi.encodeCall(SignalService.init, (address(0), sharedAddressManager)),
        //     registerTo: sharedAddressManager
        // });

        // address brdige = deployProxy({
        //     name: "bridge",
        //     impl: address(new Bridge()),
        //     data: abi.encodeCall(Bridge.init, (address(0), sharedAddressManager)),
        //     registerTo: sharedAddressManager
        // });

        // if (vm.envBool("PAUSE_BRIDGE")) {
        //     Bridge(payable(brdige)).pause();
        // }

        // Bridge(payable(brdige)).transferOwnership(owner);

        // console2.log("------------------------------------------");
        // console2.log(
        //     "Warning - you need to register *all* counterparty bridges to enable multi-hop bridging:"
        // );
        // console2.log(
        //     "sharedAddressManager.setAddress(remoteChainId, \"bridge\", address(remoteBridge))"
        // );
        // console2.log("- sharedAddressManager : ", sharedAddressManager);

        // // Deploy Vaults
        // deployProxy({
        //     name: "erc20_vault",
        //     impl: address(new ERC20Vault()),
        //     data: abi.encodeCall(ERC20Vault.init, (owner, sharedAddressManager)),
        //     registerTo: sharedAddressManager
        // });

        // deployProxy({
        //     name: "erc721_vault",
        //     impl: address(new ERC721Vault()),
        //     data: abi.encodeCall(ERC721Vault.init, (owner, sharedAddressManager)),
        //     registerTo: sharedAddressManager
        // });

        // deployProxy({
        //     name: "erc1155_vault",
        //     impl: address(new ERC1155Vault()),
        //     data: abi.encodeCall(ERC1155Vault.init, (owner, sharedAddressManager)),
        //     registerTo: sharedAddressManager
        // });

        // console2.log("------------------------------------------");
        // console2.log(
        //     "Warning - you need to register *all* counterparty vaults to enable multi-hop bridging:"
        // );
        // console2.log(
        //     "sharedAddressManager.setAddress(remoteChainId, \"erc20_vault\", address(remoteERC20Vault))"
        // );
        // console2.log(
        //     "sharedAddressManager.setAddress(remoteChainId, \"erc721_vault\", address(remoteERC721Vault))"
        // );
        // console2.log(
        //     "sharedAddressManager.setAddress(remoteChainId, \"erc1155_vault\", address(remoteERC1155Vault))"
        // );
        // console2.log("- sharedAddressManager : ", sharedAddressManager);

        // // Deploy Bridged token implementations
        // register(sharedAddressManager, "bridged_erc20", address(new BridgedERC20()));
        // register(sharedAddressManager, "bridged_erc721", address(new BridgedERC721()));
        // register(sharedAddressManager, "bridged_erc1155", address(new BridgedERC1155()));
    }

    function deployRollupContracts(
        address _sharedAddressManager,
        address owner
    )
        internal
        returns (address rollupAddressManager)
    {
        addressNotNull(_sharedAddressManager, "sharedAddressManager");
        addressNotNull(owner, "owner");

        rollupAddressManager = deployProxy({
            name: "rollup_address_manager",
            impl: address(new AddressManager()),
            data: abi.encodeCall(AddressManager.init, (owner))
        });

        // ---------------------------------------------------------------
        // Register shared contracts in the new rollup
        copyRegister(rollupAddressManager, _sharedAddressManager, "taiko_token");
        // Not deployed yet, so not needed:
        // copyRegister(rollupAddressManager, _sharedAddressManager, "signal_service");
        // copyRegister(rollupAddressManager, _sharedAddressManager, "bridge");

        deployProxy({
            name: "taiko",
            impl: address(new TaikoL1()),
            data: abi.encodeCall(
                TaikoL1.init,
                (
                    owner,
                    rollupAddressManager,
                    vm.envBytes32("L2_GENESIS_HASH")
                )
            ),
            registerTo: rollupAddressManager
        });

        /* Deploy BasedOperator */
        deployProxy({
                name: "operator",
                impl: address(new BasedOperator()),
                data: abi.encodeCall(BasedOperator.init, (MAINNET_CONTRACT_OWNER, rollupAddressManager)),
                registerTo: rollupAddressManager
        });

        /* Deploy MockSGXVerifier 3 times for now, so that we can call verifyProof without modifications of the protocol code. Later obv. shall be replaced with real verifiers. */
        address verifier1 = deployProxy({
            name: "tier_sgx1",
            impl: address(new MockSgxVerifier()),
            data: abi.encodeCall(MockSgxVerifier.init, (MAINNET_CONTRACT_OWNER, rollupAddressManager)),
            registerTo: rollupAddressManager
        });
        address verifier2 = deployProxy({
            name: "tier_sgx2",
            impl: address(new MockSgxVerifier()),
            data: abi.encodeCall(MockSgxVerifier.init, (MAINNET_CONTRACT_OWNER, rollupAddressManager)),
            registerTo: rollupAddressManager
        });
        address verifier3 = deployProxy({
            name: "tier_sgx3",
            impl: address(new MockSgxVerifier()),
            data: abi.encodeCall(MockSgxVerifier.init, (MAINNET_CONTRACT_OWNER, rollupAddressManager)),
            registerTo: rollupAddressManager
        });

        /* Deploy VerifierRegistry */
        address vieriferRegistry = deployProxy({
                name: "verifier_registry",
                impl: address(new VerifierRegistry()),
                data: abi.encodeCall(VerifierRegistry.init, (MAINNET_CONTRACT_OWNER, rollupAddressManager)),
                registerTo: rollupAddressManager
            });

        // Add those 3 to verifier registry
        VerifierRegistry(vieriferRegistry).addVerifier(verifier1, "sgx1");
        VerifierRegistry(vieriferRegistry).addVerifier(verifier2, "sgx2");
        VerifierRegistry(vieriferRegistry).addVerifier(verifier3, "sgx3");

        // Leave out guardians "tier" for now.
        // address guardianProverImpl = address(new GuardianProver());

        // address guardianProverMinority = deployProxy({
        //     name: "guardian_prover_minority",
        //     impl: guardianProverImpl,
        //     data: abi.encodeCall(GuardianProver.init, (address(0), rollupAddressManager))
        // });

        // GuardianProver(guardianProverMinority).enableTaikoTokenAllowance(true);

        // address guardianProver = deployProxy({
        //     name: "guardian_prover",
        //     impl: guardianProverImpl,
        //     data: abi.encodeCall(GuardianProver.init, (address(0), rollupAddressManager))
        // });

        // register(rollupAddressManager, "tier_guardian_minority", guardianProverMinority);
        // register(rollupAddressManager, "tier_guardian", guardianProver);
        // register(
        //     rollupAddressManager,
        //     "tier_router",
        //     address(deployTierProvider(vm.envString("TIER_PROVIDER")))
        // );

        // address[] memory guardians = vm.envAddress("GUARDIAN_PROVERS", ",");

        // GuardianProver(guardianProverMinority).setGuardians(
        //     guardians, uint8(NUM_MIN_MINORITY_GUARDIANS), true
        // );
        // GuardianProver(guardianProverMinority).transferOwnership(owner);

        // GuardianProver(guardianProver).setGuardians(
        //     guardians, uint8(NUM_MIN_MAJORITY_GUARDIANS), true
        // );
        // GuardianProver(guardianProver).transferOwnership(owner);

        // // No need to proxy these, because they are 3rd party. If we want to modify, we simply
        // // change the registerAddress("automata_dcap_attestation", address(attestation));
        // P256Verifier p256Verifier = new P256Verifier();
        // SigVerifyLib sigVerifyLib = new SigVerifyLib(address(p256Verifier));
        // PEMCertChainLib pemCertChainLib = new PEMCertChainLib();
        // address automateDcapV3AttestationImpl = address(new AutomataDcapV3Attestation());

        // address automataProxy = deployProxy({
        //     name: "automata_dcap_attestation",
        //     impl: automateDcapV3AttestationImpl,
        //     data: abi.encodeCall(
        //         AutomataDcapV3Attestation.init, (owner, address(sigVerifyLib), address(pemCertChainLib))
        //     ),
        //     registerTo: rollupAddressManager
        // });

        // // Log addresses for the user to register sgx instance
        // console2.log("SigVerifyLib", address(sigVerifyLib));
        // console2.log("PemCertChainLib", address(pemCertChainLib));
        // console2.log("AutomataDcapVaAttestation", automataProxy);

        // deployProxy({
        //     name: "prover_set",
        //     impl: address(new ProverSet()),
        //     data: abi.encodeCall(
        //         ProverSet.init, (owner, vm.envAddress("PROVER_SET_ADMIN"), rollupAddressManager)
        //     )
        // });
    }

    function addressNotNull(address addr, string memory err) private pure {
        require(addr != address(0), err);
    }
}
