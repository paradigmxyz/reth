import { setLengthLeft, toBuffer } from '@nomicfoundation/ethereumjs-util'
import { ripemd160 } from 'ethereum-cryptography/ripemd160'

import { OOGResult } from '../evm'

import type { ExecResult } from '../evm'
import type { PrecompileInput } from './types'

export function precompile03(opts: PrecompileInput): ExecResult {
  const data = opts.data

  let gasUsed = opts._common.param('gasPrices', 'ripemd160')
  gasUsed += opts._common.param('gasPrices', 'ripemd160Word') * BigInt(Math.ceil(data.length / 32))

  if (opts.gasLimit < gasUsed) {
    return OOGResult(opts.gasLimit)
  }

  return {
    executionGasUsed: gasUsed,
    returnValue: setLengthLeft(toBuffer(ripemd160(data)), 32),
  }
}
