import { OOGResult } from '../evm'

import type { ExecResult } from '../evm'
import type { PrecompileInput } from './types'

const bn128 = require('rustbn.js')

export function precompile06(opts: PrecompileInput): ExecResult {
  const inputData = opts.data

  const gasUsed = opts._common.param('gasPrices', 'ecAdd')
  if (opts.gasLimit < gasUsed) {
    return OOGResult(opts.gasLimit)
  }

  const returnData = bn128.add(inputData)

  // check ecadd success or failure by comparing the output length
  if (returnData.length !== 64) {
    return OOGResult(opts.gasLimit)
  }

  return {
    executionGasUsed: gasUsed,
    returnValue: returnData,
  }
}
