//SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

/// @title TCBInfoStruct
/// @custom:security-contact security@taiko.xyz
library TCBInfoStruct {
    struct TCBInfo {
        string pceid;
        string fmspc;
        TCBLevelObj[] tcbLevels;
    }

    struct TCBLevelObj {
        uint256 pcesvn;
        uint8[] sgxTcbCompSvnArr;
        TCBStatus status;
    }

    enum TCBStatus {
        OK,
        TCB_SW_HARDENING_NEEDED,
        TCB_CONFIGURATION_AND_SW_HARDENING_NEEDED,
        TCB_CONFIGURATION_NEEDED,
        TCB_OUT_OF_DATE,
        TCB_OUT_OF_DATE_CONFIGURATION_NEEDED,
        TCB_REVOKED,
        TCB_UNRECOGNIZED
    }
}
