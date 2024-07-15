// SPDX-License-Identifier: MIT

pragma solidity 0.8.24;

/// @title X509DateUtils
/// @custom:security-contact security@taiko.xyz
library X509DateUtils {
    function toTimestamp(bytes memory x509Time) internal pure returns (uint256) {
        uint16 yrs;
        uint8 mnths;
        uint8 dys;
        uint8 hrs;
        uint8 mins;
        uint8 secs;
        uint8 offset;

        if (x509Time.length == 13) {
            if (uint8(x509Time[0]) - 48 < 5) yrs += 2000;
            else yrs += 1900;
        } else {
            yrs += (uint8(x509Time[0]) - 48) * 1000 + (uint8(x509Time[1]) - 48) * 100;
            offset = 2;
        }
        yrs += (uint8(x509Time[offset + 0]) - 48) * 10 + uint8(x509Time[offset + 1]) - 48;
        mnths = (uint8(x509Time[offset + 2]) - 48) * 10 + uint8(x509Time[offset + 3]) - 48;
        dys += (uint8(x509Time[offset + 4]) - 48) * 10 + uint8(x509Time[offset + 5]) - 48;
        hrs += (uint8(x509Time[offset + 6]) - 48) * 10 + uint8(x509Time[offset + 7]) - 48;
        mins += (uint8(x509Time[offset + 8]) - 48) * 10 + uint8(x509Time[offset + 9]) - 48;
        secs += (uint8(x509Time[offset + 10]) - 48) * 10 + uint8(x509Time[offset + 11]) - 48;

        return toUnixTimestamp(yrs, mnths, dys, hrs, mins, secs);
    }

    function toUnixTimestamp(
        uint16 year,
        uint8 month,
        uint8 day,
        uint8 hour,
        uint8 minute,
        uint8 second
    )
        internal
        pure
        returns (uint256)
    {
        uint256 timestamp = 0;

        for (uint16 i = 1970; i < year; ++i) {
            if (isLeapYear(i)) {
                timestamp += 31_622_400; // Leap year in seconds
            } else {
                timestamp += 31_536_000; // Normal year in seconds
            }
        }

        uint8[12] memory monthDays = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        if (isLeapYear(year)) monthDays[1] = 29;

        for (uint8 i = 1; i < month; ++i) {
            timestamp += uint256(monthDays[i - 1]) * 86_400; // Days in seconds
        }

        timestamp += uint256(day - 1) * 86_400; // Days in seconds
        timestamp += uint256(hour) * 3600; // Hours in seconds
        timestamp += uint256(minute) * 60; // Minutes in seconds
        timestamp += second;

        return timestamp;
    }

    function isLeapYear(uint16 year) internal pure returns (bool) {
        if (year % 4 != 0) return false;
        if (year % 100 != 0) return true;
        if (year % 400 != 0) return false;
        return true;
    }
}
