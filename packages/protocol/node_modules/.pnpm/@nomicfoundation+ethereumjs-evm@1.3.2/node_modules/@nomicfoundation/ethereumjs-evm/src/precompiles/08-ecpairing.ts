import { OOGResult } from '../evm'

import type { ExecResult } from '../evm'
import type { PrecompileInput } from './types'

const bn128 = require('rustbn.js')

export function precompile08(opts: PrecompileInput): ExecResult {
  const inputData = opts.data
  // no need to care about non-divisible-by-192, because bn128.pairing will properly fail in that case
  const inputDataSize = BigInt(Math.floor(inputData.length / 192))
  const gasUsed =
    opts._common.param('gasPrices', 'ecPairing') +
    inputDataSize * opts._common.param('gasPrices', 'ecPairingWord')

  if (opts.gasLimit < gasUsed) {
    return OOGResult(opts.gasLimit)
  }

  const returnData = bn128.pairing(inputData)

  // check ecpairing success or failure by comparing the output length
  if (returnData.length !== 32) {
    return OOGResult(opts.gasLimit)
  }

  return {
    executionGasUsed: gasUsed,
    returnValue: returnData,
  }
}
