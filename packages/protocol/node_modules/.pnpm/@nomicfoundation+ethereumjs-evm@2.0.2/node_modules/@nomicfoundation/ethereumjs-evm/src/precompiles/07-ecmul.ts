import { OOGResult } from '../evm'

import type { ExecResult } from '../evm'
import type { PrecompileInput } from './types'

const bn128 = require('rustbn.js')

export function precompile07(opts: PrecompileInput): ExecResult {
  const inputData = opts.data
  const gasUsed = opts._common.param('gasPrices', 'ecMul')

  if (opts.gasLimit < gasUsed) {
    return OOGResult(opts.gasLimit)
  }

  const returnData = bn128.mul(inputData)
  // check ecmul success or failure by comparing the output length
  if (returnData.length !== 64) {
    return OOGResult(opts.gasLimit)
  }

  return {
    executionGasUsed: gasUsed,
    returnValue: returnData,
  }
}
