import { bufferToBigInt, toBuffer } from '@nomicfoundation/ethereumjs-util'
import { JsonRpcProvider } from '@ethersproject/providers'

import { FeeMarketEIP1559Transaction } from './eip1559Transaction'
import { AccessListEIP2930Transaction } from './eip2930Transaction'
import { normalizeTxParams } from './fromRpc'
import { Transaction } from './legacyTransaction'

import type {
  AccessListEIP2930TxData,
  FeeMarketEIP1559TxData,
  TxData,
  TxOptions,
  TypedTransaction,
} from './types'

export class TransactionFactory {
  // It is not possible to instantiate a TransactionFactory object.
  private constructor() {}

  /**
   * Create a transaction from a `txData` object
   *
   * @param txData - The transaction data. The `type` field will determine which transaction type is returned (if undefined, creates a legacy transaction)
   * @param txOptions - Options to pass on to the constructor of the transaction
   */
  public static fromTxData(
    txData: TxData | AccessListEIP2930TxData | FeeMarketEIP1559TxData,
    txOptions: TxOptions = {}
  ): TypedTransaction {
    if (!('type' in txData) || txData.type === undefined) {
      // Assume legacy transaction
      return Transaction.fromTxData(<TxData>txData, txOptions)
    } else {
      const txType = Number(bufferToBigInt(toBuffer(txData.type)))
      if (txType === 0) {
        return Transaction.fromTxData(<TxData>txData, txOptions)
      } else if (txType === 1) {
        return AccessListEIP2930Transaction.fromTxData(<AccessListEIP2930TxData>txData, txOptions)
      } else if (txType === 2) {
        return FeeMarketEIP1559Transaction.fromTxData(<FeeMarketEIP1559TxData>txData, txOptions)
      } else {
        throw new Error(`Tx instantiation with type ${txType} not supported`)
      }
    }
  }

  /**
   * This method tries to decode serialized data.
   *
   * @param data - The data Buffer
   * @param txOptions - The transaction options
   */
  public static fromSerializedData(data: Buffer, txOptions: TxOptions = {}): TypedTransaction {
    if (data[0] <= 0x7f) {
      // Determine the type.
      switch (data[0]) {
        case 1:
          return AccessListEIP2930Transaction.fromSerializedTx(data, txOptions)
        case 2:
          return FeeMarketEIP1559Transaction.fromSerializedTx(data, txOptions)
        default:
          throw new Error(`TypedTransaction with ID ${data[0]} unknown`)
      }
    } else {
      return Transaction.fromSerializedTx(data, txOptions)
    }
  }

  /**
   * When decoding a BlockBody, in the transactions field, a field is either:
   * A Buffer (a TypedTransaction - encoded as TransactionType || rlp(TransactionPayload))
   * A Buffer[] (Legacy Transaction)
   * This method returns the right transaction.
   *
   * @param data - A Buffer or Buffer[]
   * @param txOptions - The transaction options
   */
  public static fromBlockBodyData(data: Buffer | Buffer[], txOptions: TxOptions = {}) {
    if (Buffer.isBuffer(data)) {
      return this.fromSerializedData(data, txOptions)
    } else if (Array.isArray(data)) {
      // It is a legacy transaction
      return Transaction.fromValuesArray(data, txOptions)
    } else {
      throw new Error('Cannot decode transaction: unknown type input')
    }
  }

  /**
   *  Method to retrieve a transaction from the provider
   * @param provider - An Ethers JsonRPCProvider
   * @param txHash - Transaction hash
   * @param txOptions - The transaction options
   * @returns the transaction specified by `txHash`
   */
  public static async fromEthersProvider(
    provider: string | JsonRpcProvider,
    txHash: string,
    txOptions?: TxOptions
  ) {
    const prov = typeof provider === 'string' ? new JsonRpcProvider(provider) : provider
    const txData = await prov.send('eth_getTransactionByHash', [txHash])
    const normedTx = normalizeTxParams(txData)
    return TransactionFactory.fromTxData(normedTx, txOptions)
  }
}
