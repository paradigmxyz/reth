import { TypeOutput, setLengthLeft, toBuffer, toType } from '@nomicfoundation/ethereumjs-util'

import type { TxData } from './types'

export const normalizeTxParams = (_txParams: any): TxData => {
  const txParams = Object.assign({}, _txParams)

  txParams.gasLimit = toType(txParams.gasLimit ?? txParams.gas, TypeOutput.BigInt)
  txParams.data = txParams.data === undefined ? txParams.input : txParams.data

  // check and convert gasPrice and value params
  txParams.gasPrice = txParams.gasPrice !== undefined ? BigInt(txParams.gasPrice) : undefined
  txParams.value = txParams.value !== undefined ? BigInt(txParams.value) : undefined

  // strict byte length checking
  txParams.to =
    txParams.to !== null && txParams.to !== undefined
      ? setLengthLeft(toBuffer(txParams.to), 20)
      : null

  txParams.v = toType(txParams.v, TypeOutput.BigInt)

  return txParams
}
