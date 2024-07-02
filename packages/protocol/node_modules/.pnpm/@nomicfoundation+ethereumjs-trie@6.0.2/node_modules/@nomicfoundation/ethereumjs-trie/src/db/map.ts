import type { BatchDBOp, DB } from '../types'

export class MapDB implements DB {
  _database: Map<string, Buffer>

  constructor(database?: Map<string, Buffer>) {
    this._database = database ?? new Map()
  }

  async get(key: Buffer): Promise<Buffer | null> {
    const result = this._database.get(key.toString('hex'))

    if (result !== undefined) {
      return result
    }

    return null
  }

  async put(key: Buffer, val: Buffer): Promise<void> {
    this._database.set(key.toString('hex'), val)
  }

  async del(key: Buffer): Promise<void> {
    this._database.delete(key.toString('hex'))
  }

  async batch(opStack: BatchDBOp[]): Promise<void> {
    for (const op of opStack) {
      if (op.type === 'del') {
        await this.del(op.key)
      }

      if (op.type === 'put') {
        await this.put(op.key, op.value)
      }
    }
  }

  copy(): DB {
    return new MapDB(this._database)
  }
}
