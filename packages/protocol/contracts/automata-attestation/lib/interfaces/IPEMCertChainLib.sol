// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

/// @title IPEMCertChainLib
/// @custom:security-contact security@taiko.xyz
interface IPEMCertChainLib {
    struct ECSha256Certificate {
        uint256 notBefore;
        uint256 notAfter;
        bytes serialNumber;
        bytes tbsCertificate;
        bytes pubKey;
        bytes signature;
        bool isPck;
        PCKCertificateField pck;
    }

    struct PCKCertificateField {
        string commonName;
        string issuerName;
        PCKTCBInfo sgxExtension;
    }

    struct PCKTCBInfo {
        string pceid;
        string fmspc;
        uint256 pcesvn;
        uint256[] sgxTcbCompSvnArr;
    }

    enum CRL {
        PCK,
        ROOT
    }

    function splitCertificateChain(
        bytes memory pemChain,
        uint256 size
    )
        external
        pure
        returns (bool success, bytes[] memory certs);

    function decodeCert(
        bytes memory der,
        bool isPckCert
    )
        external
        pure
        returns (bool success, ECSha256Certificate memory cert);
}
