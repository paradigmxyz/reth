import {
  HEADS_KEY,
  HEAD_BLOCK_KEY,
  HEAD_HEADER_KEY,
  bodyKey,
  hashToNumberKey,
  headerKey,
  numberToHashKey,
  tdKey,
} from './constants'

import type { CacheMap } from './manager'

export enum DBTarget {
  Heads,
  HeadHeader,
  HeadBlock,
  HashToNumber,
  NumberToHash,
  TotalDifficulty,
  Body,
  Header,
  CliqueSignerStates,
  CliqueVotes,
  CliqueBlockSigners,
}

/**
 * DBOpData is a type which has the purpose of holding the actual data of the Database Operation.
 * @hidden
 */
export interface DBOpData {
  type?: string
  key: Buffer | string
  keyEncoding: string
  valueEncoding?: string
  value?: Buffer | object
}

// a Database Key is identified by a block hash, a block number, or both
export type DatabaseKey = {
  blockNumber?: bigint
  blockHash?: Buffer
}

/**
 * The DBOp class aids creating database operations which is used by `level` using a more high-level interface
 */
export class DBOp {
  public operationTarget: DBTarget
  public baseDBOp: DBOpData
  public cacheString: string | undefined

  private constructor(operationTarget: DBTarget, key?: DatabaseKey) {
    this.operationTarget = operationTarget

    this.baseDBOp = {
      key: '',
      keyEncoding: 'buffer',
      valueEncoding: 'buffer',
    }

    switch (operationTarget) {
      case DBTarget.Heads: {
        this.baseDBOp.key = HEADS_KEY
        this.baseDBOp.valueEncoding = 'json'
        break
      }
      case DBTarget.HeadHeader: {
        this.baseDBOp.key = HEAD_HEADER_KEY
        break
      }
      case DBTarget.HeadBlock: {
        this.baseDBOp.key = HEAD_BLOCK_KEY
        break
      }
      case DBTarget.HashToNumber: {
        this.baseDBOp.key = hashToNumberKey(key!.blockHash!)
        this.cacheString = 'hashToNumber'
        break
      }
      case DBTarget.NumberToHash: {
        this.baseDBOp.key = numberToHashKey(key!.blockNumber!)
        this.cacheString = 'numberToHash'
        break
      }
      case DBTarget.TotalDifficulty: {
        this.baseDBOp.key = tdKey(key!.blockNumber!, key!.blockHash!)
        this.cacheString = 'td'
        break
      }
      case DBTarget.Body: {
        this.baseDBOp.key = bodyKey(key!.blockNumber!, key!.blockHash!)
        this.cacheString = 'body'
        break
      }
      case DBTarget.Header: {
        this.baseDBOp.key = headerKey(key!.blockNumber!, key!.blockHash!)
        this.cacheString = 'header'
        break
      }
    }
  }

  public static get(operationTarget: DBTarget, key?: DatabaseKey): DBOp {
    return new DBOp(operationTarget, key)
  }

  // set operation: note: value/key is not in default order
  public static set(operationTarget: DBTarget, value: Buffer | object, key?: DatabaseKey): DBOp {
    const dbOperation = new DBOp(operationTarget, key)
    dbOperation.baseDBOp.value = value
    dbOperation.baseDBOp.type = 'put'

    if (operationTarget === DBTarget.Heads) {
      dbOperation.baseDBOp.valueEncoding = 'json'
    } else {
      dbOperation.baseDBOp.valueEncoding = 'binary'
    }

    return dbOperation
  }

  public static del(operationTarget: DBTarget, key?: DatabaseKey): DBOp {
    const dbOperation = new DBOp(operationTarget, key)
    dbOperation.baseDBOp.type = 'del'
    return dbOperation
  }

  public updateCache(cacheMap: CacheMap) {
    if (this.cacheString !== undefined && cacheMap[this.cacheString] !== undefined) {
      if (this.baseDBOp.type === 'put') {
        Buffer.isBuffer(this.baseDBOp.value) &&
          cacheMap[this.cacheString].set(this.baseDBOp.key, this.baseDBOp.value)
      } else if (this.baseDBOp.type === 'del') {
        cacheMap[this.cacheString].del(this.baseDBOp.key)
      } else {
        throw new Error('unsupported db operation on cache')
      }
    }
  }
}
