// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import { LibString } from "solady/src/utils/LibString.sol";
import { Asn1Decode, NodePtr } from "../utils/Asn1Decode.sol";
import { BytesUtils } from "../utils/BytesUtils.sol";
import { X509DateUtils } from "../utils/X509DateUtils.sol";
import { IPEMCertChainLib } from "./interfaces/IPEMCertChainLib.sol";

/// @title PEMCertChainLib
/// @custom:security-contact security@taiko.xyz
contract PEMCertChainLib is IPEMCertChainLib {
    using Asn1Decode for bytes;
    using NodePtr for uint256;
    using BytesUtils for bytes;

    string internal constant HEADER = "-----BEGIN CERTIFICATE-----";
    string internal constant FOOTER = "-----END CERTIFICATE-----";
    uint256 internal constant HEADER_LENGTH = 27;
    uint256 internal constant FOOTER_LENGTH = 25;

    string internal constant PCK_COMMON_NAME = "Intel SGX PCK Certificate";
    string internal constant PLATFORM_ISSUER_NAME = "Intel SGX PCK Platform CA";
    string internal constant PROCESSOR_ISSUER_NAME = "Intel SGX PCK Processor CA";
    bytes internal constant SGX_EXTENSION_OID = hex"2A864886F84D010D01";
    bytes internal constant TCB_OID = hex"2A864886F84D010D0102";
    bytes internal constant PCESVN_OID = hex"2A864886F84D010D010211";
    bytes internal constant PCEID_OID = hex"2A864886F84D010D0103";
    bytes internal constant FMSPC_OID = hex"2A864886F84D010D0104";

    // https://github.com/intel/SGXDataCenterAttestationPrimitives/blob/e7604e02331b3377f3766ed3653250e03af72d45/QuoteVerification/QVL/Src/AttestationLibrary/src/CertVerification/X509Constants.h#L64
    uint256 constant SGX_TCB_CPUSVN_SIZE = 16;

    struct PCKTCBFlags {
        bool fmspcFound;
        bool pceidFound;
        bool tcbFound;
    }

    function splitCertificateChain(
        bytes memory pemChain,
        uint256 size
    )
        external
        pure
        returns (bool success, bytes[] memory certs)
    {
        certs = new bytes[](size);
        string memory pemChainStr = string(pemChain);

        uint256 index = 0;
        uint256 len = pemChain.length;

        for (uint256 i; i < size; ++i) {
            string memory input;
            if (i != 0) {
                input = LibString.slice(pemChainStr, index, index + len);
            } else {
                input = pemChainStr;
            }
            uint256 increment;
            (success, certs[i], increment) = _removeHeadersAndFooters(input);

            if (!success) {
                return (false, certs);
            }

            index += increment;
        }

        success = true;
    }

    function decodeCert(
        bytes memory der,
        bool isPckCert
    )
        external
        pure
        returns (bool success, ECSha256Certificate memory cert)
    {
        uint256 root = der.root();

        // Entering tbsCertificate sequence
        uint256 tbsParentPtr = der.firstChildOf(root);

        // Begin iterating through the descendants of tbsCertificate
        uint256 tbsPtr = der.firstChildOf(tbsParentPtr);

        // The Serial Number is located one element below Version

        // The issuer commonName value is contained in the Issuer sequence
        // which is 3 elements below the first element of the tbsCertificate sequence

        // The Validity sequence is located 4 elements below the first element of the tbsCertificate
        // sequence

        // The subject commanName value is contained in the Subject sequence
        // which is 5 elements below the first element of the tbsCertificate sequence

        // The PublicKey is located in the second element of subjectPublicKeyInfo sequence
        // which is 6 elements below the first element of the tbsCertificate sequence

        tbsPtr = der.nextSiblingOf(tbsPtr);

        {
            bytes memory serialNumBytes = der.bytesAt(tbsPtr);
            cert.serialNumber = serialNumBytes;
        }

        tbsPtr = der.nextSiblingOf(tbsPtr);
        tbsPtr = der.nextSiblingOf(tbsPtr);

        if (isPckCert) {
            uint256 issuerPtr = der.firstChildOf(tbsPtr);
            issuerPtr = der.firstChildOf(issuerPtr);
            issuerPtr = der.firstChildOf(issuerPtr);
            issuerPtr = der.nextSiblingOf(issuerPtr);
            cert.pck.issuerName = string(der.bytesAt(issuerPtr));
            bool issuerNameIsValid = LibString.eq(cert.pck.issuerName, PLATFORM_ISSUER_NAME)
                || LibString.eq(cert.pck.issuerName, PROCESSOR_ISSUER_NAME);
            if (!issuerNameIsValid) {
                return (false, cert);
            }
        }

        tbsPtr = der.nextSiblingOf(tbsPtr);

        {
            uint256 notBeforePtr = der.firstChildOf(tbsPtr);
            uint256 notAfterPtr = der.nextSiblingOf(notBeforePtr);
            bytes1 notBeforeTag = der[notBeforePtr.ixs()];
            bytes1 notAfterTag = der[notAfterPtr.ixs()];
            if (
                (notBeforeTag != 0x17 && notBeforeTag != 0x18)
                    || (notAfterTag != 0x17 && notAfterTag != 0x18)
            ) {
                return (false, cert);
            }
            cert.notBefore = X509DateUtils.toTimestamp(der.bytesAt(notBeforePtr));
            cert.notAfter = X509DateUtils.toTimestamp(der.bytesAt(notAfterPtr));
        }

        tbsPtr = der.nextSiblingOf(tbsPtr);

        if (isPckCert) {
            uint256 subjectPtr = der.firstChildOf(tbsPtr);
            subjectPtr = der.firstChildOf(subjectPtr);
            subjectPtr = der.firstChildOf(subjectPtr);
            subjectPtr = der.nextSiblingOf(subjectPtr);
            cert.pck.commonName = string(der.bytesAt(subjectPtr));
            if (!LibString.eq(cert.pck.commonName, PCK_COMMON_NAME)) {
                return (false, cert);
            }
        }

        tbsPtr = der.nextSiblingOf(tbsPtr);

        {
            // Entering subjectPublicKeyInfo sequence
            uint256 subjectPublicKeyInfoPtr = der.firstChildOf(tbsPtr);
            subjectPublicKeyInfoPtr = der.nextSiblingOf(subjectPublicKeyInfoPtr);

            // The Signature sequence is located two sibling elements below the tbsCertificate
            // element
            uint256 sigPtr = der.nextSiblingOf(tbsParentPtr);
            sigPtr = der.nextSiblingOf(sigPtr);

            // Skip three bytes to the right
            // the three bytes in question: 0x034700 or 0x034800 or 0x034900
            sigPtr = NodePtr.getPtr(sigPtr.ixs() + 3, sigPtr.ixf() + 3, sigPtr.ixl());

            sigPtr = der.firstChildOf(sigPtr);
            bytes memory sigX = _trimBytes(der.bytesAt(sigPtr), 32);

            sigPtr = der.nextSiblingOf(sigPtr);
            bytes memory sigY = _trimBytes(der.bytesAt(sigPtr), 32);

            cert.tbsCertificate = der.allBytesAt(tbsParentPtr);
            cert.pubKey = _trimBytes(der.bytesAt(subjectPublicKeyInfoPtr), 64);
            cert.signature = abi.encodePacked(sigX, sigY);
        }

        if (isPckCert) {
            // entering Extension sequence
            tbsPtr = der.nextSiblingOf(tbsPtr);

            // check for the extension tag
            if (der[tbsPtr.ixs()] != 0xA3) {
                return (false, cert);
            }

            tbsPtr = der.firstChildOf(tbsPtr);
            tbsPtr = der.firstChildOf(tbsPtr);

            bool sgxExtnTraversedSuccessfully;
            uint256 pcesvn;
            uint256[] memory cpuSvns;
            bytes memory fmspcBytes;
            bytes memory pceidBytes;
            (sgxExtnTraversedSuccessfully, pcesvn, cpuSvns, fmspcBytes, pceidBytes) =
                _findPckTcbInfo(der, tbsPtr, tbsParentPtr);
            if (!sgxExtnTraversedSuccessfully) {
                return (false, cert);
            }
            cert.pck.sgxExtension.pcesvn = pcesvn;
            cert.pck.sgxExtension.sgxTcbCompSvnArr = cpuSvns;
            cert.pck.sgxExtension.pceid = LibString.toHexStringNoPrefix(pceidBytes);
            cert.pck.sgxExtension.fmspc = LibString.toHexStringNoPrefix(fmspcBytes);
            cert.isPck = true;
        }

        success = true;
    }

    function _removeHeadersAndFooters(string memory pemData)
        private
        pure
        returns (bool success, bytes memory extracted, uint256 endIndex)
    {
        // Check if the input contains the "BEGIN" and "END" headers
        uint256 beginPos = LibString.indexOf(pemData, HEADER);
        uint256 endPos = LibString.indexOf(pemData, FOOTER);

        bool headerFound = beginPos != LibString.NOT_FOUND;
        bool footerFound = endPos != LibString.NOT_FOUND;

        if (!headerFound || !footerFound) {
            return (false, extracted, endIndex);
        }

        // Extract the content between the headers
        uint256 contentStart = beginPos + HEADER_LENGTH;

        // Extract and return the content
        bytes memory contentBytes;

        // do not include newline
        bytes memory delimiter = hex"0a";
        string memory contentSlice = LibString.slice(pemData, contentStart, endPos);
        string[] memory split = LibString.split(contentSlice, string(delimiter));
        string memory contentStr;

        for (uint256 i; i < split.length; ++i) {
            contentStr = LibString.concat(contentStr, split[i]);
        }

        contentBytes = bytes(contentStr);
        return (true, contentBytes, endPos + FOOTER_LENGTH);
    }

    function _trimBytes(
        bytes memory input,
        uint256 expectedLength
    )
        private
        pure
        returns (bytes memory output)
    {
        uint256 n = input.length;

        if (n <= expectedLength) {
            return input;
        }
        uint256 lengthDiff = n - expectedLength;
        output = input.substring(lengthDiff, expectedLength);
    }

    function _findPckTcbInfo(
        bytes memory der,
        uint256 tbsPtr,
        uint256 tbsParentPtr
    )
        private
        pure
        returns (
            bool success,
            uint256 pcesvn,
            uint256[] memory cpusvns,
            bytes memory fmspcBytes,
            bytes memory pceidBytes
        )
    {
        // iterate through the elements in the Extension sequence
        // until we locate the SGX Extension OID
        while (tbsPtr != 0) {
            uint256 internalPtr = der.firstChildOf(tbsPtr);
            if (der[internalPtr.ixs()] != 0x06) {
                return (false, pcesvn, cpusvns, fmspcBytes, pceidBytes);
            }

            if (BytesUtils.compareBytes(der.bytesAt(internalPtr), SGX_EXTENSION_OID)) {
                // 1.2.840.113741.1.13.1
                internalPtr = der.nextSiblingOf(internalPtr);
                uint256 extnValueParentPtr = der.rootOfOctetStringAt(internalPtr);
                uint256 extnValuePtr = der.firstChildOf(extnValueParentPtr);

                // Copy flags to memory to avoid stack too deep
                PCKTCBFlags memory flags;

                while (!(flags.fmspcFound && flags.pceidFound && flags.tcbFound)) {
                    uint256 extnValueOidPtr = der.firstChildOf(extnValuePtr);
                    if (der[extnValueOidPtr.ixs()] != 0x06) {
                        return (false, pcesvn, cpusvns, fmspcBytes, pceidBytes);
                    }
                    if (BytesUtils.compareBytes(der.bytesAt(extnValueOidPtr), TCB_OID)) {
                        // 1.2.840.113741.1.13.1.2
                        (flags.tcbFound, pcesvn, cpusvns) = _findTcb(der, extnValueOidPtr);
                    }
                    if (BytesUtils.compareBytes(der.bytesAt(extnValueOidPtr), PCEID_OID)) {
                        // 1.2.840.113741.1.13.1.3
                        uint256 pceidPtr = der.nextSiblingOf(extnValueOidPtr);
                        pceidBytes = der.bytesAt(pceidPtr);
                        flags.pceidFound = true;
                    }
                    if (BytesUtils.compareBytes(der.bytesAt(extnValueOidPtr), FMSPC_OID)) {
                        // 1.2.840.113741.1.13.1.4
                        uint256 fmspcPtr = der.nextSiblingOf(extnValueOidPtr);
                        fmspcBytes = der.bytesAt(fmspcPtr);
                        flags.fmspcFound = true;
                    }

                    if (extnValuePtr.ixl() < extnValueParentPtr.ixl()) {
                        extnValuePtr = der.nextSiblingOf(extnValuePtr);
                    } else {
                        break;
                    }
                }
                success = flags.fmspcFound && flags.pceidFound && flags.tcbFound;
                break;
            }

            if (tbsPtr.ixl() < tbsParentPtr.ixl()) {
                tbsPtr = der.nextSiblingOf(tbsPtr);
            } else {
                tbsPtr = 0; // exit
            }
        }
    }

    function _findTcb(
        bytes memory der,
        uint256 oidPtr
    )
        private
        pure
        returns (bool success, uint256 pcesvn, uint256[] memory cpusvns)
    {
        // sibling of tcbOid
        uint256 tcbPtr = der.nextSiblingOf(oidPtr);
        // get the first svn object in the sequence
        uint256 svnParentPtr = der.firstChildOf(tcbPtr);
        cpusvns = new uint256[](SGX_TCB_CPUSVN_SIZE);
        for (uint256 i; i < SGX_TCB_CPUSVN_SIZE + 1; ++i) {
            uint256 svnPtr = der.firstChildOf(svnParentPtr); // OID
            uint256 svnValuePtr = der.nextSiblingOf(svnPtr); // value
            bytes memory svnValueBytes = der.bytesAt(svnValuePtr);
            uint16 svnValue = svnValueBytes.length < 2
                ? uint16(bytes2(svnValueBytes)) / 256
                : uint16(bytes2(svnValueBytes));
            if (BytesUtils.compareBytes(der.bytesAt(svnPtr), PCESVN_OID)) {
                // pcesvn is 4 bytes in size
                pcesvn = uint256(svnValue);
            } else {
                // each cpusvn is at maximum two bytes in size
                uint256 cpusvn = uint256(svnValue);
                cpusvns[i] = cpusvn;
            }

            // iterate to the next svn object in the sequence
            svnParentPtr = der.nextSiblingOf(svnParentPtr);
        }
        success = true;
    }
}
