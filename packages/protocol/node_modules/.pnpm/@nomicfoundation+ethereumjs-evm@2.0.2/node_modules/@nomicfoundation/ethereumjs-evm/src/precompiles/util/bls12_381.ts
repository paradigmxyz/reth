import { bufferToBigInt, padToEven } from '@nomicfoundation/ethereumjs-util'

import { ERROR, EvmError } from '../../exceptions'

// base field modulus as described in the EIP
const fieldModulus = BigInt(
  '0x1a0111ea397fe69a4b1ba7b6434bacd764774b84f38512bf6730d2a0f6b0f6241eabfffeb153ffffb9feffffffffaaab'
)

// gas discount pairs taken from EIP-2537 `Bls12381MultiExpGasDiscount` parameter
export const gasDiscountPairs = [
  [1, 1200],
  [2, 888],
  [3, 764],
  [4, 641],
  [5, 594],
  [6, 547],
  [7, 500],
  [8, 453],
  [9, 438],
  [10, 423],
  [11, 408],
  [12, 394],
  [13, 379],
  [14, 364],
  [15, 349],
  [16, 334],
  [17, 330],
  [18, 326],
  [19, 322],
  [20, 318],
  [21, 314],
  [22, 310],
  [23, 306],
  [24, 302],
  [25, 298],
  [26, 294],
  [27, 289],
  [28, 285],
  [29, 281],
  [30, 277],
  [31, 273],
  [32, 269],
  [33, 268],
  [34, 266],
  [35, 265],
  [36, 263],
  [37, 262],
  [38, 260],
  [39, 259],
  [40, 257],
  [41, 256],
  [42, 254],
  [43, 253],
  [44, 251],
  [45, 250],
  [46, 248],
  [47, 247],
  [48, 245],
  [49, 244],
  [50, 242],
  [51, 241],
  [52, 239],
  [53, 238],
  [54, 236],
  [55, 235],
  [56, 233],
  [57, 232],
  [58, 231],
  [59, 229],
  [60, 228],
  [61, 226],
  [62, 225],
  [63, 223],
  [64, 222],
  [65, 221],
  [66, 220],
  [67, 219],
  [68, 219],
  [69, 218],
  [70, 217],
  [71, 216],
  [72, 216],
  [73, 215],
  [74, 214],
  [75, 213],
  [76, 213],
  [77, 212],
  [78, 211],
  [79, 211],
  [80, 210],
  [81, 209],
  [82, 208],
  [83, 208],
  [84, 207],
  [85, 206],
  [86, 205],
  [87, 205],
  [88, 204],
  [89, 203],
  [90, 202],
  [91, 202],
  [92, 201],
  [93, 200],
  [94, 199],
  [95, 199],
  [96, 198],
  [97, 197],
  [98, 196],
  [99, 196],
  [100, 195],
  [101, 194],
  [102, 193],
  [103, 193],
  [104, 192],
  [105, 191],
  [106, 191],
  [107, 190],
  [108, 189],
  [109, 188],
  [110, 188],
  [111, 187],
  [112, 186],
  [113, 185],
  [114, 185],
  [115, 184],
  [116, 183],
  [117, 182],
  [118, 182],
  [119, 181],
  [120, 180],
  [121, 179],
  [122, 179],
  [123, 178],
  [124, 177],
  [125, 176],
  [126, 176],
  [127, 175],
  [128, 174],
]
// convert an input Buffer to a mcl G1 point
// this does /NOT/ do any input checks. the input Buffer needs to be of length 128
// it does raise an error if the point is not on the curve.
function BLS12_381_ToG1Point(input: Buffer, mcl: any): any {
  const p_x = input.slice(16, 64).toString('hex')
  const p_y = input.slice(80, 128).toString('hex')

  const ZeroString48Bytes = '0'.repeat(96)
  if (p_x === p_y && p_x === ZeroString48Bytes) {
    return new mcl.G1()
  }

  const Fp_X = new mcl.Fp()
  const Fp_Y = new mcl.Fp()
  const One = new mcl.Fp()

  Fp_X.setStr(p_x, 16)
  Fp_Y.setStr(p_y, 16)
  One.setStr('1', 16)

  const G1 = new mcl.G1()

  G1.setX(Fp_X)
  G1.setY(Fp_Y)
  G1.setZ(One)

  if (G1.isValidOrder() === false) {
    throw new EvmError(ERROR.BLS_12_381_POINT_NOT_ON_CURVE)
  }

  // Check if these coordinates are actually on the curve.
  if (G1.isValid() === false) {
    throw new EvmError(ERROR.BLS_12_381_POINT_NOT_ON_CURVE)
  }

  return G1
}

// input: a mcl G1 point
// output: a 128-byte Buffer
function BLS12_381_FromG1Point(input: any): Buffer {
  // TODO: figure out if there is a better way to decode these values.
  const decodeStr = input.getStr(16) //return a string of pattern "1 <x_coord> <y_coord>"
  const decoded = decodeStr.match(/"?[0-9a-f]+"?/g) // match above pattern.

  if (decodeStr === '0') {
    return Buffer.alloc(128, 0)
  }

  // note: decoded[0] === 1
  const xval = padToEven(decoded[1])
  const yval = padToEven(decoded[2])

  // convert to buffers.

  const xBuffer = Buffer.concat([Buffer.alloc(64 - xval.length / 2, 0), Buffer.from(xval, 'hex')])
  const yBuffer = Buffer.concat([Buffer.alloc(64 - yval.length / 2, 0), Buffer.from(yval, 'hex')])

  return Buffer.concat([xBuffer, yBuffer])
}

// convert an input Buffer to a mcl G2 point
// this does /NOT/ do any input checks. the input Buffer needs to be of length 256
function BLS12_381_ToG2Point(input: Buffer, mcl: any): any {
  const p_x_1 = input.slice(0, 64)
  const p_x_2 = input.slice(64, 128)
  const p_y_1 = input.slice(128, 192)
  const p_y_2 = input.slice(192, 256)

  const ZeroBytes64 = Buffer.alloc(64, 0)
  // check if we have to do with a zero point
  if (
    p_x_1.equals(p_x_2) &&
    p_x_1.equals(p_y_1) &&
    p_x_1.equals(p_y_2) &&
    p_x_1.equals(ZeroBytes64)
  ) {
    return new mcl.G2()
  }

  const Fp2X = BLS12_381_ToFp2Point(p_x_1, p_x_2, mcl)
  const Fp2Y = BLS12_381_ToFp2Point(p_y_1, p_y_2, mcl)

  const FpOne = new mcl.Fp()
  FpOne.setStr('1', 16)

  const FpZero = new mcl.Fp()
  FpZero.setStr('0', 16)

  const Fp2One = new mcl.Fp2()

  Fp2One.set_a(FpOne)
  Fp2One.set_b(FpZero)

  const mclPoint = new mcl.G2()

  mclPoint.setX(Fp2X)
  mclPoint.setY(Fp2Y)
  mclPoint.setZ(Fp2One)

  if (mclPoint.isValidOrder() === false) {
    throw new EvmError(ERROR.BLS_12_381_POINT_NOT_ON_CURVE)
  }

  if (mclPoint.isValid() === false) {
    throw new EvmError(ERROR.BLS_12_381_POINT_NOT_ON_CURVE)
  }

  return mclPoint
}

// input: a mcl G2 point
// output: a 256-byte Buffer
function BLS12_381_FromG2Point(input: any): Buffer {
  // TODO: figure out if there is a better way to decode these values.
  const decodeStr = input.getStr(16) //return a string of pattern "1 <x_coord_1> <x_coord_2> <y_coord_1> <y_coord_2>"
  if (decodeStr === '0') {
    return Buffer.alloc(256, 0)
  }
  const decoded = decodeStr.match(/"?[0-9a-f]+"?/g) // match above pattern.

  // note: decoded[0] === 1
  const x_1 = padToEven(decoded[1])
  const x_2 = padToEven(decoded[2])
  const y_1 = padToEven(decoded[3])
  const y_2 = padToEven(decoded[4])

  // convert to buffers.

  const xBuffer1 = Buffer.concat([Buffer.alloc(64 - x_1.length / 2, 0), Buffer.from(x_1, 'hex')])
  const xBuffer2 = Buffer.concat([Buffer.alloc(64 - x_2.length / 2, 0), Buffer.from(x_2, 'hex')])
  const yBuffer1 = Buffer.concat([Buffer.alloc(64 - y_1.length / 2, 0), Buffer.from(y_1, 'hex')])
  const yBuffer2 = Buffer.concat([Buffer.alloc(64 - y_2.length / 2, 0), Buffer.from(y_2, 'hex')])

  return Buffer.concat([xBuffer1, xBuffer2, yBuffer1, yBuffer2])
}

// input: a 32-byte hex scalar Buffer
// output: a mcl Fr point

function BLS12_381_ToFrPoint(input: Buffer, mcl: any): any {
  const mclHex = mcl.fromHexStr(input.toString('hex'))
  const Fr = new mcl.Fr()
  Fr.setBigEndianMod(mclHex)
  return Fr
}

// input: a 64-byte buffer
// output: a mcl Fp point

function BLS12_381_ToFpPoint(fpCoordinate: Buffer, mcl: any): any {
  // check if point is in field
  if (bufferToBigInt(fpCoordinate) >= fieldModulus) {
    throw new EvmError(ERROR.BLS_12_381_FP_NOT_IN_FIELD)
  }

  const fp = new mcl.Fp()

  fp.setBigEndianMod(mcl.fromHexStr(fpCoordinate.toString('hex')))

  return fp
}

// input: two 64-byte buffers
// output: a mcl Fp2 point

function BLS12_381_ToFp2Point(fpXCoordinate: Buffer, fpYCoordinate: Buffer, mcl: any): any {
  // check if the coordinates are in the field
  if (bufferToBigInt(fpXCoordinate) >= fieldModulus) {
    throw new EvmError(ERROR.BLS_12_381_FP_NOT_IN_FIELD)
  }
  if (bufferToBigInt(fpYCoordinate) >= fieldModulus) {
    throw new EvmError(ERROR.BLS_12_381_FP_NOT_IN_FIELD)
  }

  const fp_x = new mcl.Fp()
  const fp_y = new mcl.Fp()

  const fp2 = new mcl.Fp2()
  fp_x.setStr(fpXCoordinate.slice(16).toString('hex'), 16)
  fp_y.setStr(fpYCoordinate.slice(16).toString('hex'), 16)

  fp2.set_a(fp_x)
  fp2.set_b(fp_y)

  return fp2
}

export {
  BLS12_381_FromG1Point,
  BLS12_381_FromG2Point,
  BLS12_381_ToFp2Point,
  BLS12_381_ToFpPoint,
  BLS12_381_ToFrPoint,
  BLS12_381_ToG1Point,
  BLS12_381_ToG2Point,
}
