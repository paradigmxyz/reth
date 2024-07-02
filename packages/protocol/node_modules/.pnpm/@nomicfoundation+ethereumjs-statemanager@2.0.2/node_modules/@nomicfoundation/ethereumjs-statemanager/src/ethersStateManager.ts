import { Trie } from '@nomicfoundation/ethereumjs-trie'
import {
  Account,
  bigIntToHex,
  bufferToBigInt,
  bufferToHex,
  setLengthLeft,
  toBuffer,
} from '@nomicfoundation/ethereumjs-util'
import { debug } from 'debug'
import { keccak256 } from 'ethereum-cryptography/keccak'
import { ethers } from 'ethers'

import { Cache } from './cache'

import { BaseStateManager } from '.'

import type { Proof, StateManager } from '.'
import type { getCb, putCb } from './cache'
import type { StorageDump } from './interface'
import type { Address } from '@nomicfoundation/ethereumjs-util'

const log = debug('statemanager')

export interface EthersStateManagerOpts {
  provider: string | ethers.providers.StaticJsonRpcProvider | ethers.providers.JsonRpcProvider
  blockTag: bigint | 'earliest'
}

export class EthersStateManager extends BaseStateManager implements StateManager {
  private provider: ethers.providers.StaticJsonRpcProvider | ethers.providers.JsonRpcProvider
  private contractCache: Map<string, Buffer>
  private storageCache: Map<string, Map<string, Buffer>>
  private blockTag: string
  _cache: Cache

  constructor(opts: EthersStateManagerOpts) {
    super({})
    if (typeof opts.provider === 'string') {
      this.provider = new ethers.providers.StaticJsonRpcProvider(opts.provider)
    } else if (opts.provider instanceof ethers.providers.JsonRpcProvider) {
      this.provider = opts.provider
    } else {
      throw new Error(`valid JsonRpcProvider or url required; got ${opts.provider}`)
    }

    this.blockTag = opts.blockTag === 'earliest' ? opts.blockTag : bigIntToHex(opts.blockTag)

    this.contractCache = new Map()
    this.storageCache = new Map()

    const getCb: getCb = async (address) => {
      return this.getAccountFromProvider(address)
    }
    const putCb: putCb = async (_keyBuf, _accountRlp) => {
      return Promise.resolve()
    }
    const deleteCb = async (_keyBuf: Buffer) => {
      return Promise.resolve()
    }
    this._cache = new Cache({ getCb, putCb, deleteCb })
  }

  copy(): EthersStateManager {
    const newState = new EthersStateManager({
      provider: this.provider,
      blockTag: BigInt(this.blockTag),
    })
    ;(newState as any).contractCache = new Map(this.contractCache)
    ;(newState as any).storageCache = new Map(this.storageCache)
    ;(newState as any)._cache = this._cache
    return newState
  }

  /**
   * Sets the new block tag used when querying the provider and clears the
   * internal cache.
   * @param blockTag - the new block tag to use when querying the provider
   */
  setBlockTag(blockTag: bigint | 'earliest'): void {
    this.blockTag = blockTag === 'earliest' ? blockTag : bigIntToHex(blockTag)
    this.clearCache()
  }

  /**
   * Clears the internal cache so all accounts, contract code, and storage slots will
   * initially be retrieved from the provider
   */
  clearCache(): void {
    this.contractCache.clear()
    this.storageCache.clear()
    this._cache.clear()
  }

  /**
   * Gets the code corresponding to the provided `address`.
   * @param address - Address to get the `code` for
   * @returns {Promise<Buffer>} - Resolves with the code corresponding to the provided address.
   * Returns an empty `Buffer` if the account has no associated code.
   */
  async getContractCode(address: Address): Promise<Buffer> {
    let codeBuffer = this.contractCache.get(address.toString())
    if (codeBuffer !== undefined) return codeBuffer
    const code = await this.provider.getCode(address.toString(), this.blockTag)
    codeBuffer = toBuffer(code)
    this.contractCache.set(address.toString(), codeBuffer)
    return codeBuffer
  }

  /**
   * Adds `value` to the state trie as code, and sets `codeHash` on the account
   * corresponding to `address` to reference this.
   * @param address - Address of the `account` to add the `code` for
   * @param value - The value of the `code`
   */
  async putContractCode(address: Address, value: Buffer): Promise<void> {
    // Store contract code in the cache
    this.contractCache.set(address.toString(), value)
  }

  /**
   * Gets the storage value associated with the provided `address` and `key`. This method returns
   * the shortest representation of the stored value.
   * @param address - Address of the account to get the storage for
   * @param key - Key in the account's storage to get the value for. Must be 32 bytes long.
   * @returns {Buffer} - The storage value for the account
   * corresponding to the provided address at the provided key.
   * If this does not exist an empty `Buffer` is returned.
   */
  async getContractStorage(address: Address, key: Buffer): Promise<Buffer> {
    // Check storage slot in cache
    const accountStorage: Map<string, Buffer> | undefined = this.storageCache.get(
      address.toString()
    )
    let storage: Buffer | string | undefined
    if (accountStorage !== undefined) {
      storage = accountStorage.get(key.toString('hex'))
      if (storage !== undefined) {
        return storage
      }
    }

    // Retrieve storage slot from provider if not found in cache
    storage = await this.provider.getStorageAt(
      address.toString(),
      bufferToBigInt(key),
      this.blockTag
    )
    const value = toBuffer(storage)

    await this.putContractStorage(address, key, value)
    return value
  }

  /**
   * Adds value to the cache for the `account`
   * corresponding to `address` at the provided `key`.
   * @param address - Address to set a storage value for
   * @param key - Key to set the value at. Must be 32 bytes long.
   * @param value - Value to set at `key` for account corresponding to `address`.
   * Cannot be more than 32 bytes. Leading zeros are stripped.
   * If it is empty or filled with zeros, deletes the value.
   */
  async putContractStorage(address: Address, key: Buffer, value: Buffer): Promise<void> {
    let accountStorage = this.storageCache.get(address.toString())
    if (accountStorage === undefined) {
      this.storageCache.set(address.toString(), new Map<string, Buffer>())
      accountStorage = this.storageCache.get(address.toString())
    }
    accountStorage?.set(key.toString('hex'), value)
  }

  /**
   * Clears all storage entries for the account corresponding to `address`.
   * @param address - Address to clear the storage of
   */
  async clearContractStorage(address: Address): Promise<void> {
    this.storageCache.delete(address.toString())
  }

  /**
   * Dumps the RLP-encoded storage values for an `account` specified by `address`.
   * @param address - The address of the `account` to return storage for
   * @returns {Promise<StorageDump>} - The state of the account as an `Object` map.
   * Keys are the storage keys, values are the storage values as strings.
   * Both are represented as `0x` prefixed hex strings.
   */
  dumpStorage(address: Address): Promise<StorageDump> {
    const addressStorage = this.storageCache.get(address.toString())
    const dump: StorageDump = {}
    if (addressStorage !== undefined) {
      for (const slot of addressStorage) {
        dump[slot[0]] = bufferToHex(slot[1])
      }
    }
    return Promise.resolve(dump)
  }

  /**
   * Checks if an `account` exists at `address`
   * @param address - Address of the `account` to check
   */
  async accountExists(address: Address): Promise<boolean> {
    log(`Verify if ${address.toString()} exists`)

    const localAccount = this._cache.get(address)
    if (!localAccount.isEmpty()) return true
    // Get merkle proof for `address` from provider
    const proof = await this.provider.send('eth_getProof', [address.toString(), [], this.blockTag])

    const proofBuf = proof.accountProof.map((proofNode: string) => toBuffer(proofNode))

    const trie = new Trie({ useKeyHashing: true })
    const verified = await trie.verifyProof(
      Buffer.from(keccak256(proofBuf[0])),
      address.buf,
      proofBuf
    )
    // if not verified (i.e. verifyProof returns null), account does not exist
    return verified === null ? false : true
  }

  /**
   * Gets the code corresponding to the provided `address`.
   * @param address - Address to get the `code` for
   * @returns {Promise<Buffer>} - Resolves with the code corresponding to the provided address.
   * Returns an empty `Buffer` if the account has no associated code.
   */
  async getAccount(address: Address): Promise<Account> {
    const account = this._cache.getOrLoad(address)

    return account
  }

  /**
   * Retrieves an account from the provider and stores in the local trie
   * @param address Address of account to be retrieved from provider
   * @private
   */
  async getAccountFromProvider(address: Address): Promise<Account> {
    const accountData = await this.provider.send('eth_getProof', [
      address.toString(),
      [],
      this.blockTag,
    ])
    const account = Account.fromAccountData({
      balance: BigInt(accountData.balance),
      nonce: BigInt(accountData.nonce),
      codeHash: toBuffer(accountData.codeHash),
      storageRoot: toBuffer(accountData.storageHash),
    })
    return account
  }

  /**
   * Saves an account into state under the provided `address`.
   * @param address - Address under which to store `account`
   * @param account - The account to store
   */
  async putAccount(address: Address, account: Account): Promise<void> {
    this._cache.put(address, account)
  }

  /**
   * Get an EIP-1186 proof from the provider
   * @param address address to get proof of
   * @param storageSlots storage slots to get proof of
   * @returns an EIP-1186 formatted proof
   */
  async getProof(address: Address, storageSlots: Buffer[] = []): Promise<Proof> {
    const proof = await this.provider.send('eth_getProof', [
      address.toString(),
      [storageSlots.map((slot) => bufferToHex(slot))],
      this.blockTag,
    ])

    return proof
  }

  /**
   * Checkpoints the current state of the StateManager instance.
   * State changes that follow can then be committed by calling
   * `commit` or `reverted` by calling rollback.
   *
   * Partial implementation, called from the subclass.
   */
  async checkpoint(): Promise<void> {
    this._cache.checkpoint()
  }

  /**
   * Commits the current change-set to the instance since the
   * last call to checkpoint.
   *
   * Partial implementation, called from the subclass.
   */
  async commit(): Promise<void> {
    // setup cache checkpointing
    this._cache.commit()
  }

  /**
   * Reverts the current change-set to the instance since the
   * last call to checkpoint.
   *
   * Partial implementation , called from the subclass.
   */
  async revert(): Promise<void> {
    // setup cache checkpointing
    this._cache.revert()
  }

  async flush(): Promise<void> {
    await this._cache.flush()
  }

  /**
   * @deprecated This method is not used by the Ethers State Manager and is a stub required by the State Manager interface
   */
  getStateRoot = async () => {
    return setLengthLeft(Buffer.from([]), 32)
  }

  /**
   * @deprecated This method is not used by the Ethers State Manager and is a stub required by the State Manager interface
   */
  setStateRoot = async (_root: Buffer) => {}

  /**
   * @deprecated This method is not used by the Ethers State Manager and is a stub required by the State Manager interface
   */
  hasStateRoot = () => {
    throw new Error('function not implemented')
  }
}
