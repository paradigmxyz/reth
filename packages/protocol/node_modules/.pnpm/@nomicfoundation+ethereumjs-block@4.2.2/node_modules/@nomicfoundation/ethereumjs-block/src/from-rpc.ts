import { TransactionFactory } from '@nomicfoundation/ethereumjs-tx'
import { setLengthLeft, toBuffer } from '@nomicfoundation/ethereumjs-util'

import { blockHeaderFromRpc } from './header-from-rpc'
import { numberToHex } from './helpers'

import { Block } from './index'

import type { BlockOptions, JsonRpcBlock } from './index'
import type { TxData, TypedTransaction } from '@nomicfoundation/ethereumjs-tx'

function normalizeTxParams(_txParams: any) {
  const txParams = Object.assign({}, _txParams)

  txParams.gasLimit = txParams.gasLimit === undefined ? txParams.gas : txParams.gasLimit
  txParams.data = txParams.data === undefined ? txParams.input : txParams.data

  // check and convert gasPrice and value params
  txParams.gasPrice = numberToHex(txParams.gasPrice)
  txParams.value = numberToHex(txParams.value)

  // strict byte length checking
  txParams.to =
    txParams.to !== null && txParams.to !== undefined
      ? setLengthLeft(toBuffer(txParams.to), 20)
      : null

  // v as raw signature value {0,1}
  // v is the recovery bit and can be either {0,1} or {27,28}.
  // https://ethereum.stackexchange.com/questions/40679/why-the-value-of-v-is-always-either-27-11011-or-28-11100
  const v: number = txParams.v
  txParams.v = v < 27 ? v + 27 : v

  return txParams
}

/**
 * Creates a new block object from Ethereum JSON RPC.
 *
 * @param blockParams - Ethereum JSON RPC of block (eth_getBlockByNumber)
 * @param uncles - Optional list of Ethereum JSON RPC of uncles (eth_getUncleByBlockHashAndIndex)
 * @param options - An object describing the blockchain
 */
export function blockFromRpc(
  blockParams: JsonRpcBlock,
  uncles: any[] = [],
  options?: BlockOptions
) {
  const header = blockHeaderFromRpc(blockParams, options)

  const transactions: TypedTransaction[] = []
  const opts = { common: header._common }
  for (const _txParams of blockParams.transactions) {
    const txParams = normalizeTxParams(_txParams)
    const tx = TransactionFactory.fromTxData(txParams as TxData, opts)
    transactions.push(tx)
  }

  const uncleHeaders = uncles.map((uh) => blockHeaderFromRpc(uh, options))

  return Block.fromBlockData({ header, transactions, uncleHeaders }, options)
}
