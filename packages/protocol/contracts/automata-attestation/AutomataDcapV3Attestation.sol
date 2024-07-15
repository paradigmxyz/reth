//SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import { V3Struct } from "./lib/QuoteV3Auth/V3Struct.sol";
import { V3Parser } from "./lib/QuoteV3Auth/V3Parser.sol";
import { IPEMCertChainLib } from "./lib/interfaces/IPEMCertChainLib.sol";
import { PEMCertChainLib } from "./lib/PEMCertChainLib.sol";
import { TCBInfoStruct } from "./lib/TCBInfoStruct.sol";
import { EnclaveIdStruct } from "./lib/EnclaveIdStruct.sol";
import { IAttestation } from "./interfaces/IAttestation.sol";

// Internal Libraries
import { Base64 } from "solady/src/utils/Base64.sol";
import { LibString } from "solady/src/utils/LibString.sol";
import { BytesUtils } from "./utils/BytesUtils.sol";

// External Libraries
import { ISigVerifyLib } from "./interfaces/ISigVerifyLib.sol";

import { EssentialContract } from "../common/EssentialContract.sol";

/// @title AutomataDcapV3Attestation
/// @custom:security-contact security@taiko.xyz
contract AutomataDcapV3Attestation is IAttestation, EssentialContract {
    using BytesUtils for bytes;

    // https://github.com/intel/SGXDataCenterAttestationPrimitives/blob/e7604e02331b3377f3766ed3653250e03af72d45/QuoteVerification/QVL/Src/AttestationLibrary/src/CertVerification/X509Constants.h#L64
    uint256 internal constant CPUSVN_LENGTH = 16;

    // keccak256(hex"0ba9c4c0c0c86193a3fe23d6b02cda10a8bbd4e88e48b4458561a36e705525f567918e2edc88e40d860bd0cc4ee26aacc988e505a953558c453f6b0904ae7394")
    // the uncompressed (0x04) prefix is not included in the pubkey pre-image
    bytes32 internal constant ROOTCA_PUBKEY_HASH =
        0x89f72d7c488e5b53a77c23ebcb36970ef7eb5bcf6658e9b8292cfbe4703a8473;

    uint8 internal constant INVALID_EXIT_CODE = 255;

    ISigVerifyLib public sigVerifyLib; // slot 1
    IPEMCertChainLib public pemCertLib; // slot 2

    bool public checkLocalEnclaveReport; // slot 3
    mapping(bytes32 enclave => bool trusted) public trustedUserMrEnclave; // slot 4
    mapping(bytes32 signer => bool trusted) public trustedUserMrSigner; // slot 5

    // Quote Collateral Configuration

    // Index definition:
    // 0 = Quote PCKCrl
    // 1 = RootCrl
    mapping(uint256 idx => mapping(bytes serialNum => bool revoked)) public serialNumIsRevoked; // slot
        // 6
    // fmspc => tcbInfo
    mapping(string fmspc => TCBInfoStruct.TCBInfo tcbInfo) public tcbInfo; // slot 7
    EnclaveIdStruct.EnclaveId public qeIdentity; // takes 4 slots, slot 8,9,10,11

    uint256[39] __gap;

    event MrSignerUpdated(bytes32 indexed mrSigner, bool trusted);
    event MrEnclaveUpdated(bytes32 indexed mrEnclave, bool trusted);
    event TcbInfoJsonConfigured(string indexed fmspc, TCBInfoStruct.TCBInfo tcbInfoInput);
    event QeIdentityConfigured(EnclaveIdStruct.EnclaveId qeIdentityInput);
    event LocalReportCheckToggled(bool checkLocalEnclaveReport);
    event RevokedCertSerialNumAdded(uint256 indexed index, bytes serialNum);
    event RevokedCertSerialNumRemoved(uint256 indexed index, bytes serialNum);

    // @notice Initializes the contract.
    /// @param sigVerifyLibAddr Address of the signature verification library.
    /// @param pemCertLibAddr Address of certificate library.
    function init(
        address owner,
        address sigVerifyLibAddr,
        address pemCertLibAddr
    )
        external
        initializer
    {
        __Essential_init(owner);
        sigVerifyLib = ISigVerifyLib(sigVerifyLibAddr);
        pemCertLib = PEMCertChainLib(pemCertLibAddr);
    }

    function setMrSigner(bytes32 _mrSigner, bool _trusted) external onlyOwner {
        trustedUserMrSigner[_mrSigner] = _trusted;
        emit MrSignerUpdated(_mrSigner, _trusted);
    }

    function setMrEnclave(bytes32 _mrEnclave, bool _trusted) external onlyOwner {
        trustedUserMrEnclave[_mrEnclave] = _trusted;
        emit MrEnclaveUpdated(_mrEnclave, _trusted);
    }

    function addRevokedCertSerialNum(
        uint256 index,
        bytes[] calldata serialNumBatch
    )
        external
        onlyOwner
    {
        for (uint256 i; i < serialNumBatch.length; ++i) {
            if (serialNumIsRevoked[index][serialNumBatch[i]]) {
                continue;
            }
            serialNumIsRevoked[index][serialNumBatch[i]] = true;
            emit RevokedCertSerialNumAdded(index, serialNumBatch[i]);
        }
    }

    function removeRevokedCertSerialNum(
        uint256 index,
        bytes[] calldata serialNumBatch
    )
        external
        onlyOwner
    {
        for (uint256 i; i < serialNumBatch.length; ++i) {
            if (!serialNumIsRevoked[index][serialNumBatch[i]]) {
                continue;
            }
            delete serialNumIsRevoked[index][serialNumBatch[i]];
            emit RevokedCertSerialNumRemoved(index, serialNumBatch[i]);
        }
    }

    function configureTcbInfoJson(
        string calldata fmspc,
        TCBInfoStruct.TCBInfo calldata tcbInfoInput
    )
        public
        onlyOwner
    {
        // 2.2M gas
        tcbInfo[fmspc] = tcbInfoInput;
        emit TcbInfoJsonConfigured(fmspc, tcbInfoInput);
    }

    function configureQeIdentityJson(EnclaveIdStruct.EnclaveId calldata qeIdentityInput)
        external
        onlyOwner
    {
        // 250k gas
        qeIdentity = qeIdentityInput;
        emit QeIdentityConfigured(qeIdentityInput);
    }

    function toggleLocalReportCheck() external onlyOwner {
        checkLocalEnclaveReport = !checkLocalEnclaveReport;
        emit LocalReportCheckToggled(checkLocalEnclaveReport);
    }

    function _attestationTcbIsValid(TCBInfoStruct.TCBStatus status)
        internal
        pure
        virtual
        returns (bool valid)
    {
        return status == TCBInfoStruct.TCBStatus.OK
            || status == TCBInfoStruct.TCBStatus.TCB_SW_HARDENING_NEEDED
            || status == TCBInfoStruct.TCBStatus.TCB_CONFIGURATION_AND_SW_HARDENING_NEEDED
            || status == TCBInfoStruct.TCBStatus.TCB_OUT_OF_DATE
            || status == TCBInfoStruct.TCBStatus.TCB_OUT_OF_DATE_CONFIGURATION_NEEDED;
    }

    function verifyAttestation(bytes calldata data) external view override returns (bool success) {
        (success,) = _verify(data);
    }

    /// @dev Provide the raw quote binary as input
    /// @dev The attestation data (or the returned data of this method)
    /// is constructed depending on the validity of the quote verification.
    /// @dev After confirming that a quote has been verified, the attestation's validity then
    /// depends on the
    /// status of the associated TCB.
    /// @dev Example scenarios as below:
    /// --------------------------------
    /// @dev Invalid quote verification: returns (false, INVALID_EXIT_CODE)
    ///
    /// @dev For all valid quote verification, the validity of the attestation depends on the status
    /// of a
    /// matching TCBInfo and this is defined in the _attestationTcbIsValid() method, which can be
    /// overwritten
    /// in derived contracts. (Except for "Revoked" status, which also returns (false,
    /// INVALID_EXIT_CODE) value)
    /// @dev For all valid quote verification, returns the following data:
    /// (_attestationTcbIsValid(), abi.encodePacked(sha256(quote), uint8 exitCode))
    /// @dev exitCode is defined in the {{ TCBInfoStruct.TCBStatus }} enum
    function _verify(bytes calldata quote) private view returns (bool, bytes memory) {
        bytes memory retData = abi.encodePacked(INVALID_EXIT_CODE);

        // Step 1: Parse the quote input = 152k gas
        (bool successful, V3Struct.ParsedV3QuoteStruct memory parsedV3Quote) =
            V3Parser.parseInput(quote, address(pemCertLib));
        if (!successful) {
            return (false, retData);
        }

        return _verifyParsedQuote(parsedV3Quote);
    }

    function _verifyQEReportWithIdentity(V3Struct.EnclaveReport memory quoteEnclaveReport)
        private
        view
        returns (bool, EnclaveIdStruct.EnclaveIdStatus status)
    {
        EnclaveIdStruct.EnclaveId memory enclaveId = qeIdentity;
        bool miscselectMatched =
            quoteEnclaveReport.miscSelect & enclaveId.miscselectMask == enclaveId.miscselect;

        bool attributesMatched =
            quoteEnclaveReport.attributes & enclaveId.attributesMask == enclaveId.attributes;
        bool mrsignerMatched = quoteEnclaveReport.mrSigner == enclaveId.mrsigner;

        bool isvprodidMatched = quoteEnclaveReport.isvProdId == enclaveId.isvprodid;

        bool tcbFound;
        for (uint256 i; i < enclaveId.tcbLevels.length; ++i) {
            EnclaveIdStruct.TcbLevel memory tcb = enclaveId.tcbLevels[i];
            if (tcb.tcb.isvsvn <= quoteEnclaveReport.isvSvn) {
                tcbFound = true;
                status = tcb.tcbStatus;
                break;
            }
        }
        return (
            miscselectMatched && attributesMatched && mrsignerMatched && isvprodidMatched
                && tcbFound,
            status
        );
    }

    function _checkTcbLevels(
        IPEMCertChainLib.PCKCertificateField memory pck,
        TCBInfoStruct.TCBInfo memory tcb
    )
        private
        pure
        returns (bool, TCBInfoStruct.TCBStatus status)
    {
        for (uint256 i; i < tcb.tcbLevels.length; ++i) {
            TCBInfoStruct.TCBLevelObj memory current = tcb.tcbLevels[i];
            bool pceSvnIsHigherOrGreater = pck.sgxExtension.pcesvn >= current.pcesvn;
            bool cpuSvnsAreHigherOrGreater = _isCpuSvnHigherOrGreater(
                pck.sgxExtension.sgxTcbCompSvnArr, current.sgxTcbCompSvnArr
            );
            if (pceSvnIsHigherOrGreater && cpuSvnsAreHigherOrGreater) {
                status = current.status;
                bool tcbIsRevoked = status == TCBInfoStruct.TCBStatus.TCB_REVOKED;
                return (!tcbIsRevoked, status);
            }
        }
        return (true, TCBInfoStruct.TCBStatus.TCB_UNRECOGNIZED);
    }

    function _isCpuSvnHigherOrGreater(
        uint256[] memory pckCpuSvns,
        uint8[] memory tcbCpuSvns
    )
        private
        pure
        returns (bool)
    {
        if (pckCpuSvns.length != CPUSVN_LENGTH || tcbCpuSvns.length != CPUSVN_LENGTH) {
            return false;
        }
        for (uint256 i; i < CPUSVN_LENGTH; ++i) {
            if (pckCpuSvns[i] < tcbCpuSvns[i]) {
                return false;
            }
        }
        return true;
    }

    function _verifyCertChain(IPEMCertChainLib.ECSha256Certificate[] memory certs)
        private
        view
        returns (bool)
    {
        uint256 n = certs.length;
        bool certRevoked;
        bool certNotExpired;
        bool verified;
        bool certChainCanBeTrusted;

        for (uint256 i; i < n; ++i) {
            IPEMCertChainLib.ECSha256Certificate memory issuer;
            if (i == n - 1) {
                // rootCA
                issuer = certs[i];
            } else {
                issuer = certs[i + 1];
                if (i == n - 2) {
                    // this cert is expected to be signed by the root
                    certRevoked = serialNumIsRevoked[uint256(IPEMCertChainLib.CRL.ROOT)][certs[i]
                        .serialNumber];
                } else if (certs[i].isPck) {
                    certRevoked =
                        serialNumIsRevoked[uint256(IPEMCertChainLib.CRL.PCK)][certs[i].serialNumber];
                }
                if (certRevoked) {
                    break;
                }
            }

            certNotExpired =
                block.timestamp > certs[i].notBefore && block.timestamp < certs[i].notAfter;
            if (!certNotExpired) {
                break;
            }

            verified = sigVerifyLib.verifyES256Signature(
                certs[i].tbsCertificate, certs[i].signature, issuer.pubKey
            );
            if (!verified) {
                break;
            }

            bytes32 issuerPubKeyHash = keccak256(issuer.pubKey);

            if (issuerPubKeyHash == ROOTCA_PUBKEY_HASH) {
                certChainCanBeTrusted = true;
                break;
            }
        }

        return !certRevoked && certNotExpired && verified && certChainCanBeTrusted;
    }

    function _enclaveReportSigVerification(
        bytes memory pckCertPubKey,
        bytes memory signedQuoteData,
        V3Struct.ECDSAQuoteV3AuthData memory authDataV3,
        V3Struct.EnclaveReport memory qeEnclaveReport
    )
        private
        view
        returns (bool)
    {
        bytes32 expectedAuthDataHash = bytes32(qeEnclaveReport.reportData.substring(0, 32));
        bytes memory concatOfAttestKeyAndQeAuthData =
            abi.encodePacked(authDataV3.ecdsaAttestationKey, authDataV3.qeAuthData.data);
        bytes32 computedAuthDataHash = sha256(concatOfAttestKeyAndQeAuthData);

        bool qeReportDataIsValid = expectedAuthDataHash == computedAuthDataHash;
        if (qeReportDataIsValid) {
            bytes memory pckSignedQeReportBytes =
                V3Parser.packQEReport(authDataV3.pckSignedQeReport);
            bool qeSigVerified = sigVerifyLib.verifyES256Signature(
                pckSignedQeReportBytes, authDataV3.qeReportSignature, pckCertPubKey
            );
            bool quoteSigVerified = sigVerifyLib.verifyES256Signature(
                signedQuoteData, authDataV3.ecdsa256BitSignature, authDataV3.ecdsaAttestationKey
            );
            return qeSigVerified && quoteSigVerified;
        } else {
            return false;
        }
    }

    /// --------------- validate parsed quote ---------------

    /// @dev Provide the parsed quote binary as input
    /// @dev The attestation data (or the returned data of this method)
    /// is constructed depending on the validity of the quote verification.
    /// @dev After confirming that a quote has been verified, the attestation's validity then
    /// depends on the
    /// status of the associated TCB.
    /// @dev Example scenarios as below:
    /// --------------------------------
    /// @dev Invalid quote verification: returns (false, INVALID_EXIT_CODE)
    ///
    /// @dev For all valid quote verification, the validity of the attestation depends on the status
    /// of a
    /// matching TCBInfo and this is defined in the _attestationTcbIsValid() method, which can be
    /// overwritten
    /// in derived contracts. (Except for "Revoked" status, which also returns (false,
    /// INVALID_EXIT_CODE) value)
    /// @dev For all valid quote verification, returns the following data:
    /// (_attestationTcbIsValid())
    /// @dev exitCode is defined in the {{ TCBInfoStruct.TCBStatus }} enum
    function verifyParsedQuote(V3Struct.ParsedV3QuoteStruct calldata v3quote)
        external
        view
        override
        returns (bool, bytes memory)
    {
        return _verifyParsedQuote(v3quote);
    }

    function _verifyParsedQuote(V3Struct.ParsedV3QuoteStruct memory v3quote)
        internal
        view
        returns (bool, bytes memory)
    {
        bytes memory retData = abi.encodePacked(INVALID_EXIT_CODE);

        // // Step 1: Parse the quote input = 152k gas
        (
            bool successful,
            ,
            ,
            bytes memory signedQuoteData,
            V3Struct.ECDSAQuoteV3AuthData memory authDataV3
        ) = V3Parser.validateParsedInput(v3quote);
        if (!successful) {
            return (false, retData);
        }

        // Step 2: Verify application enclave report MRENCLAVE and MRSIGNER
        {
            if (checkLocalEnclaveReport) {
                // 4k gas
                bool mrEnclaveIsTrusted = trustedUserMrEnclave[v3quote.localEnclaveReport.mrEnclave];
                bool mrSignerIsTrusted = trustedUserMrSigner[v3quote.localEnclaveReport.mrSigner];

                if (!mrEnclaveIsTrusted || !mrSignerIsTrusted) {
                    return (false, retData);
                }
            }
        }

        // Step 3: Verify enclave identity = 43k gas
        EnclaveIdStruct.EnclaveIdStatus qeTcbStatus;
        {
            bool verifiedEnclaveIdSuccessfully;
            (verifiedEnclaveIdSuccessfully, qeTcbStatus) =
                _verifyQEReportWithIdentity(v3quote.v3AuthData.pckSignedQeReport);
            if (!verifiedEnclaveIdSuccessfully) {
                return (false, retData);
            }
            if (
                !verifiedEnclaveIdSuccessfully
                    || qeTcbStatus == EnclaveIdStruct.EnclaveIdStatus.SGX_ENCLAVE_REPORT_ISVSVN_REVOKED
            ) {
                return (false, retData);
            }
        }

        // Step 4: Parse Quote CertChain
        IPEMCertChainLib.ECSha256Certificate[] memory parsedQuoteCerts;
        TCBInfoStruct.TCBInfo memory fetchedTcbInfo;
        {
            // 536k gas
            parsedQuoteCerts = new IPEMCertChainLib.ECSha256Certificate[](3);
            for (uint256 i; i < 3; ++i) {
                bool isPckCert = i == 0; // additional parsing for PCKCert
                bool certDecodedSuccessfully;
                // todo! move decodeCert offchain
                (certDecodedSuccessfully, parsedQuoteCerts[i]) = pemCertLib.decodeCert(
                    authDataV3.certification.decodedCertDataArray[i], isPckCert
                );
                if (!certDecodedSuccessfully) {
                    return (false, retData);
                }
            }
        }

        // Step 5: basic PCK and TCB check = 381k gas
        {
            string memory parsedFmspc = parsedQuoteCerts[0].pck.sgxExtension.fmspc;
            fetchedTcbInfo = tcbInfo[parsedFmspc];
            bool tcbConfigured = LibString.eq(parsedFmspc, fetchedTcbInfo.fmspc);
            if (!tcbConfigured) {
                return (false, retData);
            }

            IPEMCertChainLib.ECSha256Certificate memory pckCert = parsedQuoteCerts[0];
            bool pceidMatched = LibString.eq(pckCert.pck.sgxExtension.pceid, fetchedTcbInfo.pceid);
            if (!pceidMatched) {
                return (false, retData);
            }
        }

        // Step 6: Verify TCB Level
        TCBInfoStruct.TCBStatus tcbStatus;
        {
            // 4k gas
            bool tcbVerified;
            (tcbVerified, tcbStatus) = _checkTcbLevels(parsedQuoteCerts[0].pck, fetchedTcbInfo);
            if (!tcbVerified) {
                return (false, retData);
            }
        }

        // Step 7: Verify cert chain for PCK
        {
            // 660k gas (rootCA pubkey is trusted)
            bool pckCertChainVerified = _verifyCertChain(parsedQuoteCerts);
            if (!pckCertChainVerified) {
                return (false, retData);
            }
        }

        // Step 8: Verify the local attestation sig and qe report sig = 670k gas
        {
            bool enclaveReportSigsVerified = _enclaveReportSigVerification(
                parsedQuoteCerts[0].pubKey,
                signedQuoteData,
                authDataV3,
                v3quote.v3AuthData.pckSignedQeReport
            );
            if (!enclaveReportSigsVerified) {
                return (false, retData);
            }
        }

        retData = abi.encodePacked(sha256(abi.encode(v3quote)), tcbStatus);

        return (_attestationTcbIsValid(tcbStatus), retData);
    }
}
