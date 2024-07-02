import { TypeOutput, isHexString, toType } from '@nomicfoundation/ethereumjs-util'

import type { BlockHeader } from './header'
import type { BlockHeaderBuffer, HeaderData } from './types'

/**
 * Returns a 0x-prefixed hex number string from a hex string or string integer.
 * @param {string} input string to check, convert, and return
 */
export const numberToHex = function (input?: string) {
  if (input === undefined) return undefined
  if (!isHexString(input)) {
    const regex = new RegExp(/^\d+$/) // test to make sure input contains only digits
    if (!regex.test(input)) {
      const msg = `Cannot convert string to hex string. numberToHex only supports 0x-prefixed hex or integer strings but the given string was: ${input}`
      throw new Error(msg)
    }
    return '0x' + parseInt(input, 10).toString(16)
  }
  return input
}

export function valuesArrayToHeaderData(values: BlockHeaderBuffer): HeaderData {
  const [
    parentHash,
    uncleHash,
    coinbase,
    stateRoot,
    transactionsTrie,
    receiptTrie,
    logsBloom,
    difficulty,
    number,
    gasLimit,
    gasUsed,
    timestamp,
    extraData,
    mixHash,
    nonce,
    baseFeePerGas,
    withdrawalsRoot,
    excessDataGas,
  ] = values

  if (values.length > 18) {
    throw new Error('invalid header. More values than expected were received')
  }
  if (values.length < 15) {
    throw new Error('invalid header. Less values than expected were received')
  }

  return {
    parentHash,
    uncleHash,
    coinbase,
    stateRoot,
    transactionsTrie,
    receiptTrie,
    logsBloom,
    difficulty,
    number,
    gasLimit,
    gasUsed,
    timestamp,
    extraData,
    mixHash,
    nonce,
    baseFeePerGas,
    withdrawalsRoot,
    excessDataGas,
  }
}

export function getDifficulty(headerData: HeaderData): bigint | null {
  const { difficulty } = headerData
  if (difficulty !== undefined) {
    return toType(difficulty, TypeOutput.BigInt)
  }
  return null
}

/**
 * Calculates the excess data gas for a post EIP 4844 block given the parent block header.
 * @param parent header for the parent block
 * @param newBlobs number of blobs contained in block
 * @returns the excess data gas for the prospective next block
 *
 * Note: This function expects that it is only being called on a valid block as it does not have
 * access to the "current" block's common instance to verify if 4844 is active or not.
 */
export const calcExcessDataGas = (parent: BlockHeader, newBlobs: number) => {
  if (!parent._common.isActivatedEIP(4844)) {
    // If 4844 isn't active on header, assume this is the first post-fork block so excess data gas is 0
    return BigInt(0)
  }
  if (parent.excessDataGas === undefined) {
    // Given 4844 is active on parent block, we expect it to have an excessDataGas field
    throw new Error('parent header does not contain excessDataGas field')
  }

  const consumedDataGas = BigInt(newBlobs) * parent._common.param('gasConfig', 'dataGasPerBlob')
  const targetDataGasPerBlock = parent._common.param('gasConfig', 'targetDataGasPerBlock')

  if (parent.excessDataGas + consumedDataGas < targetDataGasPerBlock) return BigInt(0)
  else {
    return parent.excessDataGas + consumedDataGas - targetDataGasPerBlock
  }
}

/**
 * Approximates `factor * e ** (numerator / denominator)` using Taylor expansion
 */
export const fakeExponential = (factor: bigint, numerator: bigint, denominator: bigint) => {
  let i = BigInt(1)
  let output = BigInt(0)
  let numerator_accum = factor * denominator
  while (numerator_accum > BigInt(0)) {
    output += numerator_accum
    numerator_accum = BigInt(Math.floor(Number((numerator_accum * numerator) / (denominator * i))))
    i++
  }
  return BigInt(Math.floor(Number(output / denominator)))
}

/**
 * Returns the price per unit of data gas for a blob transaction in the current/pending block
 * @param header the parent header for the current block (or current head of the chain)
 * @returns the price in gwei per unit of data gas spent
 */
export const getDataGasPrice = (header: BlockHeader) => {
  if (header.excessDataGas === undefined) {
    throw new Error('parent header must have excessDataGas field populated')
  }
  return fakeExponential(
    header._common.param('gasPrices', 'minDataGasPrice'),
    header.excessDataGas,
    header._common.param('gasConfig', 'dataGasPriceUpdateFraction')
  )
}

/**
 * Returns the total fee for data gas spent on `numBlobs` in the current/pending block
 * @param numBlobs
 * @param parent parent header of the current/pending block
 * @returns the total data gas fee for a transaction assuming it contains `numBlobs`
 */
export const calcDataFee = (numBlobs: number, parent: BlockHeader) => {
  if (parent.excessDataGas === undefined) {
    throw new Error('parent header must have excessDataGas field populated')
  }
  const totalDataGas = parent._common.param('gasConfig', 'dataGasPerBlob') * BigInt(numBlobs)
  const dataGasPrice = getDataGasPrice(parent)
  return totalDataGas * dataGasPrice
}
