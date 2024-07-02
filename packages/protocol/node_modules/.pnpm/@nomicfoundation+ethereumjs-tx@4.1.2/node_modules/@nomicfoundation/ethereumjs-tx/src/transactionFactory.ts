import { bufferToBigInt, toBuffer } from '@nomicfoundation/ethereumjs-util'

import { FeeMarketEIP1559Transaction } from './eip1559Transaction'
import { AccessListEIP2930Transaction } from './eip2930Transaction'
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
      let EIP: number
      switch (data[0]) {
        case 1:
          EIP = 2930
          break
        case 2:
          EIP = 1559
          break
        default:
          throw new Error(`TypedTransaction with ID ${data[0]} unknown`)
      }
      if (EIP === 1559) {
        return FeeMarketEIP1559Transaction.fromSerializedTx(data, txOptions)
      } else {
        // EIP === 2930
        return AccessListEIP2930Transaction.fromSerializedTx(data, txOptions)
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
}
