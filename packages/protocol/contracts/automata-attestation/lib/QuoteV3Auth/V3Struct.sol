//SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

/// @title V3Struct
/// @custom:security-contact security@taiko.xyz
library V3Struct {
    struct Header {
        bytes2 version;
        bytes2 attestationKeyType;
        bytes4 teeType;
        bytes2 qeSvn;
        bytes2 pceSvn;
        bytes16 qeVendorId;
        bytes20 userData;
    }

    struct EnclaveReport {
        bytes16 cpuSvn;
        bytes4 miscSelect;
        bytes28 reserved1;
        bytes16 attributes;
        bytes32 mrEnclave;
        bytes32 reserved2;
        bytes32 mrSigner;
        bytes reserved3; // 96 bytes
        uint16 isvProdId;
        uint16 isvSvn;
        bytes reserved4; // 60 bytes
        bytes reportData; // 64 bytes - For QEReports, this contains the hash of the concatenation
            // of attestation key and QEAuthData
    }

    struct QEAuthData {
        uint16 parsedDataSize;
        bytes data;
    }

    struct CertificationData {
        uint16 certType;
        // todo! In encoded path, we need to calculate the size of certDataArray
        // certDataSize = len(join((BEGIN_CERT, certArray[i], END_CERT) for i in 0..3))
        // But for plain bytes path, we don't need that.
        uint32 certDataSize;
        bytes[3] decodedCertDataArray; // base64 decoded cert bytes array
    }

    struct ECDSAQuoteV3AuthData {
        bytes ecdsa256BitSignature; // 64 bytes
        bytes ecdsaAttestationKey; // 64 bytes
        EnclaveReport pckSignedQeReport; // 384 bytes
        bytes qeReportSignature; // 64 bytes
        QEAuthData qeAuthData;
        CertificationData certification;
    }

    struct ParsedV3QuoteStruct {
        Header header;
        EnclaveReport localEnclaveReport;
        ECDSAQuoteV3AuthData v3AuthData;
    }
}
