//SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

/// @title EnclaveIdStruct
/// @custom:security-contact security@taiko.xyz
library EnclaveIdStruct {
    struct EnclaveId {
        bytes4 miscselect; // Slot 1:
        bytes4 miscselectMask;
        uint16 isvprodid;
        bytes16 attributes; // Slot 2
        bytes16 attributesMask;
        bytes32 mrsigner; // Slot 3
        TcbLevel[] tcbLevels; // Slot 4
    }

    struct TcbLevel {
        TcbObj tcb;
        EnclaveIdStatus tcbStatus;
    }

    struct TcbObj {
        uint16 isvsvn;
    }

    enum EnclaveIdStatus {
        OK,
        SGX_ENCLAVE_REPORT_ISVSVN_REVOKED
    }
}
