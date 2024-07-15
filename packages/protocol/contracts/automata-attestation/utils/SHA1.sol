// SPDX-License-Identifier: BSD 2-Clause License

pragma solidity 0.8.24;

// Inspired by ensdomains/solsha1 - BSD 2-Clause License
// https://github.com/ensdomains/solsha1/blob/master/contracts/SHA1.sol

/// @title SHA1
/// @custom:security-contact security@taiko.xyz
library SHA1 {
    function sha1(bytes memory data) internal pure returns (bytes20 ret) {
        assembly {
            // Get a safe scratch location
            let scratch := mload(0x40)

            // Get the data length, and point data at the first byte
            let len := mload(data)
            data := add(data, 32)

            // Find the length after padding
            let totallen := add(and(add(len, 1), 0xFFFFFFFFFFFFFFC0), 64)
            switch lt(sub(totallen, len), 9)
            case 1 { totallen := add(totallen, 64) }

            let h := 0x6745230100EFCDAB890098BADCFE001032547600C3D2E1F0

            function readword(ptr, off, count) -> result {
                result := 0
                if lt(off, count) {
                    result := mload(add(ptr, off))
                    count := sub(count, off)
                    if lt(count, 32) {
                        let mask := not(sub(exp(256, sub(32, count)), 1))
                        result := and(result, mask)
                    }
                }
            }

            for { let i := 0 } lt(i, totallen) { i := add(i, 64) } {
                mstore(scratch, readword(data, i, len))
                mstore(add(scratch, 32), readword(data, add(i, 32), len))

                // If we loaded the last byte, store the terminator byte
                switch lt(sub(len, i), 64)
                case 1 { mstore8(add(scratch, sub(len, i)), 0x80) }

                // If this is the last block, store the length
                switch eq(i, sub(totallen, 64))
                case 1 { mstore(add(scratch, 32), or(mload(add(scratch, 32)), mul(len, 8))) }

                // Expand the 16 32-bit words into 80
                for { let j := 64 } lt(j, 128) { j := add(j, 12) } {
                    let temp :=
                        xor(
                            xor(mload(add(scratch, sub(j, 12))), mload(add(scratch, sub(j, 32)))),
                            xor(mload(add(scratch, sub(j, 56))), mload(add(scratch, sub(j, 64))))
                        )
                    temp :=
                        or(
                            and(
                                mul(temp, 2),
                                0xFFFFFFFEFFFFFFFEFFFFFFFEFFFFFFFEFFFFFFFEFFFFFFFEFFFFFFFEFFFFFFFE
                            ),
                            and(
                                div(temp, 0x80000000),
                                0x0000000100000001000000010000000100000001000000010000000100000001
                            )
                        )
                    mstore(add(scratch, j), temp)
                }
                for { let j := 128 } lt(j, 320) { j := add(j, 24) } {
                    let temp :=
                        xor(
                            xor(mload(add(scratch, sub(j, 24))), mload(add(scratch, sub(j, 64)))),
                            xor(mload(add(scratch, sub(j, 112))), mload(add(scratch, sub(j, 128))))
                        )
                    temp :=
                        or(
                            and(
                                mul(temp, 4),
                                0xFFFFFFFCFFFFFFFCFFFFFFFCFFFFFFFCFFFFFFFCFFFFFFFCFFFFFFFCFFFFFFFC
                            ),
                            and(
                                div(temp, 0x40000000),
                                0x0000000300000003000000030000000300000003000000030000000300000003
                            )
                        )
                    mstore(add(scratch, j), temp)
                }

                let x := h
                let f := 0
                let k := 0
                for { let j := 0 } lt(j, 80) { j := add(j, 1) } {
                    switch div(j, 20)
                    case 0 {
                        // f = d xor (b and (c xor d))
                        f := xor(div(x, 0x100000000000000000000), div(x, 0x10000000000))
                        f := and(div(x, 0x1000000000000000000000000000000), f)
                        f := xor(div(x, 0x10000000000), f)
                        k := 0x5A827999
                    }
                    case 1 {
                        // f = b xor c xor d
                        f :=
                            xor(
                                div(x, 0x1000000000000000000000000000000),
                                div(x, 0x100000000000000000000)
                            )
                        f := xor(div(x, 0x10000000000), f)
                        k := 0x6ED9EBA1
                    }
                    case 2 {
                        // f = (b and c) or (d and (b or c))
                        f :=
                            or(
                                div(x, 0x1000000000000000000000000000000),
                                div(x, 0x100000000000000000000)
                            )
                        f := and(div(x, 0x10000000000), f)
                        f :=
                            or(
                                and(
                                    div(x, 0x1000000000000000000000000000000),
                                    div(x, 0x100000000000000000000)
                                ),
                                f
                            )
                        k := 0x8F1BBCDC
                    }
                    case 3 {
                        // f = b xor c xor d
                        f :=
                            xor(
                                div(x, 0x1000000000000000000000000000000),
                                div(x, 0x100000000000000000000)
                            )
                        f := xor(div(x, 0x10000000000), f)
                        k := 0xCA62C1D6
                    }
                    // temp = (a leftrotate 5) + f + e + k + w[i]
                    let temp := and(div(x, 0x80000000000000000000000000000000000000000000000), 0x1F)
                    temp :=
                        or(and(div(x, 0x800000000000000000000000000000000000000), 0xFFFFFFE0), temp)
                    temp := add(f, temp)
                    temp := add(and(x, 0xFFFFFFFF), temp)
                    temp := add(k, temp)
                    temp :=
                        add(
                            div(
                                mload(add(scratch, mul(j, 4))),
                                0x100000000000000000000000000000000000000000000000000000000
                            ),
                            temp
                        )
                    x :=
                        or(
                            div(x, 0x10000000000),
                            mul(temp, 0x10000000000000000000000000000000000000000)
                        )
                    x :=
                        or(
                            and(x, 0xFFFFFFFF00FFFFFFFF000000000000FFFFFFFF00FFFFFFFF),
                            mul(
                                or(
                                    and(div(x, 0x4000000000000), 0xC0000000),
                                    and(div(x, 0x400000000000000000000), 0x3FFFFFFF)
                                ),
                                0x100000000000000000000
                            )
                        )
                }

                h := and(add(h, x), 0xFFFFFFFF00FFFFFFFF00FFFFFFFF00FFFFFFFF00FFFFFFFF)
            }
            ret :=
                mul(
                    or(
                        or(
                            or(
                                or(
                                    and(div(h, 0x100000000), 0xFFFFFFFF00000000000000000000000000000000),
                                    and(div(h, 0x1000000), 0xFFFFFFFF000000000000000000000000)
                                ),
                                and(div(h, 0x10000), 0xFFFFFFFF0000000000000000)
                            ),
                            and(div(h, 0x100), 0xFFFFFFFF00000000)
                        ),
                        and(h, 0xFFFFFFFF)
                    ),
                    0x1000000000000000000000000
                )
        }
    }
}
