import { Account } from '@nomicfoundation/ethereumjs-util'
import { OrderedMap } from 'js-sdsl'

import type { Address } from '@nomicfoundation/ethereumjs-util'
import type { OrderedMapIterator } from 'js-sdsl'

export type getCb = (address: Address) => Promise<Account | undefined>
export type putCb = (keyBuf: Buffer, accountRlp: Buffer) => Promise<void>
export type deleteCb = (keyBuf: Buffer) => Promise<void>

export interface CacheOpts {
  getCb: getCb
  putCb: putCb
  deleteCb: deleteCb
}

/**
 * @ignore
 */
export class Cache {
  _cache: OrderedMap<any, any>
  _cacheEnd: OrderedMapIterator<any, any>
  _checkpoints: any[]

  _getCb: getCb
  _putCb: putCb
  _deleteCb: deleteCb

  constructor(opts: CacheOpts) {
    this._cache = new OrderedMap()
    this._cacheEnd = this._cache.end()
    this._getCb = opts.getCb
    this._putCb = opts.putCb
    this._deleteCb = opts.deleteCb
    this._checkpoints = []
  }

  /**
   * Puts account to cache under its address.
   * @param key - Address of account
   * @param val - Account
   */
  put(key: Address, val: Account, fromTrie: boolean = false): void {
    const modified = !fromTrie
    this._update(key, val, modified, false)
  }

  /**
   * Returns the queried account or an empty account.
   * @param key - Address of account
   */
  get(key: Address): Account {
    const account = this.lookup(key)
    return account ?? new Account()
  }

  /**
   * Returns the queried account or undefined.
   * @param key - Address of account
   */
  lookup(key: Address): Account | undefined {
    const keyStr = key.buf.toString('hex')

    const it = this._cache.find(keyStr)
    if (!it.equals(this._cacheEnd)) {
      const value = it.pointer[1]
      const rlp = value.val
      const account = Account.fromRlpSerializedAccount(rlp)
      ;(account as any).virtual = value.virtual
      return account
    }
  }

  /**
   * Returns true if the key was deleted and thus existed in the cache earlier
   * @param key - trie key to lookup
   */
  keyIsDeleted(key: Address): boolean {
    const keyStr = key.buf.toString('hex')
    const it = this._cache.find(keyStr)
    if (!it.equals(this._cacheEnd)) {
      return it.pointer[1].deleted
    }
    return false
  }

  /**
   * Looks up address in cache, if not found, looks it up
   * in the underlying trie.
   * @param key - Address of account
   */
  async getOrLoad(address: Address): Promise<Account> {
    let account = this.lookup(address)

    if (!account) {
      account = await this._getCb(address)
      if (account) {
        this._update(address, account, false, false, false)
      } else {
        account = new Account()
        ;(account as any).virtual = true
        this._update(address, account, false, false, true)
      }
    }

    return account
  }

  /**
   * Flushes cache by updating accounts that have been modified
   * and removing accounts that have been deleted.
   */
  async flush(): Promise<void> {
    const it = this._cache.begin()
    while (!it.equals(this._cacheEnd)) {
      const value = it.pointer[1]
      if (value.modified === true) {
        value.modified = false
        const keyBuf = Buffer.from(it.pointer[0], 'hex')
        if (value.deleted === false) {
          const accountRlp = value.val
          await this._putCb(keyBuf, accountRlp)
        } else {
          value.deleted = true
          value.virtual = true
          value.val = new Account().serialize()
          await this._deleteCb(keyBuf)
        }
      }
      it.next()
    }
  }

  /**
   * Marks current state of cache as checkpoint, which can
   * later on be reverted or committed.
   */
  checkpoint(): void {
    this._checkpoints.push(new OrderedMap(this._cache))
  }

  /**
   * Revert changes to cache last checkpoint (no effect on trie).
   */
  revert(): void {
    this._cache = this._checkpoints.pop()
    this._cacheEnd = this._cache.end()
  }

  /**
   * Commits to current state of cache (no effect on trie).
   */
  commit(): void {
    this._checkpoints.pop()
  }

  /**
   * Clears cache.
   */
  clear(): void {
    this._cache.clear()
  }

  /**
   * Marks address as deleted in cache.
   * @param key - Address
   */
  del(key: Address): void {
    this._update(key, new Account(), true, true, true)
  }

  /**
   * Generic cache update helper function
   *
   * @param key
   * @param value
   * @param modified - Has the value been modified or is it coming unchanged from the trie (also used for deleted accounts)
   * @param deleted - Delete operation on an account
   * @param virtual - Account doesn't exist in the underlying trie
   */
  _update(
    key: Address,
    value: Account,
    modified: boolean,
    deleted: boolean,
    virtual = false
  ): void {
    const keyHex = key.buf.toString('hex')
    const val = value.serialize()
    this._cache.setElement(keyHex, { val, modified, deleted, virtual })
  }
}
