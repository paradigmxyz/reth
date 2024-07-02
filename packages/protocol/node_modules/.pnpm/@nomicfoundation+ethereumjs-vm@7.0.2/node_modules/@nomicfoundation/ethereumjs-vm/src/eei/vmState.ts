import { Chain, Common, Hardfork } from '@nomicfoundation/ethereumjs-common'
import { ripemdPrecompileAddress } from '@nomicfoundation/ethereumjs-evm/dist/precompiles'
import { Account, Address, toBuffer } from '@nomicfoundation/ethereumjs-util'
import { debug as createDebugLogger } from 'debug'

import type { EVMStateAccess } from '@nomicfoundation/ethereumjs-evm/dist/types'
import type { AccountFields, StateManager } from '@nomicfoundation/ethereumjs-statemanager'
import type { AccessList, AccessListItem } from '@nomicfoundation/ethereumjs-tx'
import type { Debugger } from 'debug'

type AddressHex = string

export class VmState implements EVMStateAccess {
  protected _common: Common
  protected _debug: Debugger

  protected _checkpointCount: number
  protected _stateManager: StateManager
  protected _touched: Set<AddressHex>
  protected _touchedStack: Set<AddressHex>[]

  // EIP-2929 address/storage trackers.
  // This maps both the accessed accounts and the accessed storage slots.
  // It is a Map(Address => StorageSlots)
  // It is possible that the storage slots set is empty. This means that the address is warm.
  // It is not possible to have an accessed storage slot on a cold address (which is why this structure works)
  // Each call level tracks their access themselves.
  // In case of a commit, copy everything if the value does not exist, to the level above
  // In case of a revert, discard any warm slots.
  protected _accessedStorage: Map<string, Set<string>>[]

  // Backup structure for address/storage tracker frames on reverts
  // to also include on access list generation
  protected _accessedStorageReverted: Map<string, Set<string>>[]

  protected _originalStorageCache: Map<AddressHex, Map<AddressHex, Buffer>>

  protected readonly DEBUG: boolean = false

  constructor({ common, stateManager }: { common?: Common; stateManager: StateManager }) {
    this._checkpointCount = 0
    this._stateManager = stateManager
    this._common = common ?? new Common({ chain: Chain.Mainnet, hardfork: Hardfork.Petersburg })
    this._touched = new Set()
    this._touchedStack = []
    this._originalStorageCache = new Map()
    this._accessedStorage = [new Map()]
    this._accessedStorageReverted = [new Map()]

    // Skip DEBUG calls unless 'ethjs' included in environmental DEBUG variables
    this.DEBUG = process?.env?.DEBUG?.includes('ethjs') ?? false

    this._debug = createDebugLogger('vm:state')
  }

  /**
   * Checkpoints the current state of the StateManager instance.
   * State changes that follow can then be committed by calling
   * `commit` or `reverted` by calling rollback.
   *
   * Partial implementation, called from the subclass.
   */
  async checkpoint(): Promise<void> {
    this._touchedStack.push(new Set(Array.from(this._touched)))
    this._accessedStorage.push(new Map())
    await this._stateManager.checkpoint()
    this._checkpointCount++

    if (this.DEBUG) {
      this._debug('-'.repeat(100))
      this._debug(`message checkpoint`)
    }
  }

  async commit(): Promise<void> {
    // setup cache checkpointing
    this._touchedStack.pop()
    // Copy the contents of the map of the current level to a map higher.
    const storageMap = this._accessedStorage.pop()
    if (storageMap) {
      this._accessedStorageMerge(this._accessedStorage, storageMap)
    }
    await this._stateManager.commit()
    this._checkpointCount--

    if (this._checkpointCount === 0) {
      await this._stateManager.flush()
      this._clearOriginalStorageCache()
    }

    if (this.DEBUG) {
      this._debug(`message checkpoint committed`)
    }
  }

  /**
   * Reverts the current change-set to the instance since the
   * last call to checkpoint.
   *
   * Partial implementation , called from the subclass.
   */
  async revert(): Promise<void> {
    // setup cache checkpointing
    const lastItem = this._accessedStorage.pop()
    if (lastItem) {
      this._accessedStorageReverted.push(lastItem)
    }
    const touched = this._touchedStack.pop()
    if (!touched) {
      throw new Error('Reverting to invalid state checkpoint failed')
    }
    // Exceptional case due to consensus issue in Geth and Parity.
    // See [EIP issue #716](https://github.com/ethereum/EIPs/issues/716) for context.
    // The RIPEMD precompile has to remain *touched* even when the call reverts,
    // and be considered for deletion.
    if (this._touched.has(ripemdPrecompileAddress)) {
      touched.add(ripemdPrecompileAddress)
    }
    this._touched = touched
    await this._stateManager.revert()

    this._checkpointCount--

    if (this._checkpointCount === 0) {
      await this._stateManager.flush()
      this._clearOriginalStorageCache()
    }

    if (this.DEBUG) {
      this._debug(`message checkpoint reverted`)
    }
  }

  async getAccount(address: Address): Promise<Account> {
    return this._stateManager.getAccount(address)
  }

  async putAccount(address: Address, account: Account): Promise<void> {
    await this._stateManager.putAccount(address, account)
    this.touchAccount(address)
  }

  async modifyAccountFields(address: Address, accountFields: AccountFields): Promise<void> {
    return this._stateManager.modifyAccountFields(address, accountFields)
  }

  /**
   * Deletes an account from state under the provided `address`. The account will also be removed from the state trie.
   * @param address - Address of the account which should be deleted
   */
  async deleteAccount(address: Address) {
    await this._stateManager.deleteAccount(address)
    this.touchAccount(address)
  }

  async getContractCode(address: Address): Promise<Buffer> {
    return this._stateManager.getContractCode(address)
  }

  async putContractCode(address: Address, value: Buffer): Promise<void> {
    return this._stateManager.putContractCode(address, value)
  }

  async getContractStorage(address: Address, key: Buffer): Promise<Buffer> {
    return this._stateManager.getContractStorage(address, key)
  }

  async putContractStorage(address: Address, key: Buffer, value: Buffer) {
    await this._stateManager.putContractStorage(address, key, value)
    this.touchAccount(address)
  }

  async clearContractStorage(address: Address) {
    await this._stateManager.clearContractStorage(address)
    this.touchAccount(address)
  }

  async accountExists(address: Address): Promise<boolean> {
    return this._stateManager.accountExists(address)
  }

  async setStateRoot(stateRoot: Buffer): Promise<void> {
    if (this._checkpointCount !== 0) {
      throw new Error('Cannot set state root with uncommitted checkpoints')
    }
    return this._stateManager.setStateRoot(stateRoot)
  }

  async getStateRoot(): Promise<Buffer> {
    return this._stateManager.getStateRoot()
  }

  async hasStateRoot(root: Buffer): Promise<boolean> {
    return this._stateManager.hasStateRoot(root)
  }

  /**
   * Marks an account as touched, according to the definition
   * in [EIP-158](https://eips.ethereum.org/EIPS/eip-158).
   * This happens when the account is triggered for a state-changing
   * event. Touched accounts that are empty will be cleared
   * at the end of the tx.
   */
  touchAccount(address: Address): void {
    this._touched.add(address.buf.toString('hex'))
  }

  /**
   * Merges a storage map into the last item of the accessed storage stack
   */
  private _accessedStorageMerge(
    storageList: Map<string, Set<string> | undefined>[],
    storageMap: Map<string, Set<string>>
  ) {
    const mapTarget = storageList[storageList.length - 1]

    if (mapTarget !== undefined) {
      // Note: storageMap is always defined here per definition (TypeScript cannot infer this)
      for (const [addressString, slotSet] of storageMap) {
        const addressExists = mapTarget.get(addressString)
        if (!addressExists) {
          mapTarget.set(addressString, new Set())
        }
        const storageSet = mapTarget.get(addressString)
        for (const value of slotSet) {
          storageSet!.add(value)
        }
      }
    }
  }

  /**
   * Initializes the provided genesis state into the state trie.
   * Will error if there are uncommitted checkpoints on the instance.
   * @param initState address -> balance | [balance, code, storage]
   */
  async generateCanonicalGenesis(initState: any): Promise<void> {
    if (this._checkpointCount !== 0) {
      throw new Error('Cannot create genesis state with uncommitted checkpoints')
    }
    if (this.DEBUG) {
      this._debug(`Save genesis state into the state trie`)
    }
    const addresses = Object.keys(initState)
    for (const address of addresses) {
      const addr = Address.fromString(address)
      const state = initState[address]
      if (!Array.isArray(state)) {
        // Prior format: address -> balance
        const account = Account.fromAccountData({ balance: state })
        await this.putAccount(addr, account)
      } else {
        // New format: address -> [balance, code, storage]
        const [balance, code, storage] = state
        const account = Account.fromAccountData({ balance })
        await this.putAccount(addr, account)
        if (code !== undefined) {
          await this.putContractCode(addr, toBuffer(code))
        }
        if (storage !== undefined) {
          for (const [key, value] of storage) {
            await this.putContractStorage(addr, toBuffer(key), toBuffer(value))
          }
        }
      }
    }
    await this._stateManager.flush()
  }

  /**
   * Removes accounts form the state trie that have been touched,
   * as defined in EIP-161 (https://eips.ethereum.org/EIPS/eip-161).
   */
  async cleanupTouchedAccounts(): Promise<void> {
    if (this._common.gteHardfork(Hardfork.SpuriousDragon) === true) {
      const touchedArray = Array.from(this._touched)
      for (const addressHex of touchedArray) {
        const address = new Address(Buffer.from(addressHex, 'hex'))
        const empty = await this.accountIsEmpty(address)
        if (empty) {
          await this._stateManager.deleteAccount(address)
          if (this.DEBUG) {
            this._debug(`Cleanup touched account address=${address} (>= SpuriousDragon)`)
          }
        }
      }
    }
    this._touched.clear()
  }

  /**
   * Caches the storage value associated with the provided `address` and `key`
   * on first invocation, and returns the cached (original) value from then
   * onwards. This is used to get the original value of a storage slot for
   * computing gas costs according to EIP-1283.
   * @param address - Address of the account to get the storage for
   * @param key - Key in the account's storage to get the value for. Must be 32 bytes long.
   */
  protected async getOriginalContractStorage(address: Address, key: Buffer): Promise<Buffer> {
    if (key.length !== 32) {
      throw new Error('Storage key must be 32 bytes long')
    }

    const addressHex = address.buf.toString('hex')
    const keyHex = key.toString('hex')

    let map: Map<AddressHex, Buffer>
    if (!this._originalStorageCache.has(addressHex)) {
      map = new Map()
      this._originalStorageCache.set(addressHex, map)
    } else {
      map = this._originalStorageCache.get(addressHex)!
    }

    if (map.has(keyHex)) {
      return map.get(keyHex)!
    } else {
      const current = await this.getContractStorage(address, key)
      map.set(keyHex, current)
      return current
    }
  }

  /**
   * Clears the original storage cache. Refer to {@link StateManager.getOriginalContractStorage}
   * for more explanation.
   */
  _clearOriginalStorageCache(): void {
    this._originalStorageCache = new Map()
  }

  /**
   * Clears the original storage cache. Refer to {@link StateManager.getOriginalContractStorage}
   * for more explanation. Alias of the internal {@link StateManager._clearOriginalStorageCache}
   */
  clearOriginalStorageCache(): void {
    this._clearOriginalStorageCache()
  }

  /** EIP-2929 logic
   * This should only be called from within the EVM
   */

  /**
   * Returns true if the address is warm in the current context
   * @param address - The address (as a Buffer) to check
   */
  isWarmedAddress(address: Buffer): boolean {
    for (let i = this._accessedStorage.length - 1; i >= 0; i--) {
      const currentMap = this._accessedStorage[i]
      if (currentMap.has(address.toString('hex'))) {
        return true
      }
    }
    return false
  }

  /**
   * Add a warm address in the current context
   * @param address - The address (as a Buffer) to check
   */
  addWarmedAddress(address: Buffer): void {
    const key = address.toString('hex')
    const storageSet = this._accessedStorage[this._accessedStorage.length - 1].get(key)
    if (!storageSet) {
      const emptyStorage = new Set<string>()
      this._accessedStorage[this._accessedStorage.length - 1].set(key, emptyStorage)
    }
  }

  /**
   * Returns true if the slot of the address is warm
   * @param address - The address (as a Buffer) to check
   * @param slot - The slot (as a Buffer) to check
   */
  isWarmedStorage(address: Buffer, slot: Buffer): boolean {
    const addressKey = address.toString('hex')
    const storageKey = slot.toString('hex')

    for (let i = this._accessedStorage.length - 1; i >= 0; i--) {
      const currentMap = this._accessedStorage[i]
      if (currentMap.has(addressKey) && currentMap.get(addressKey)!.has(storageKey)) {
        return true
      }
    }

    return false
  }

  /**
   * Mark the storage slot in the address as warm in the current context
   * @param address - The address (as a Buffer) to check
   * @param slot - The slot (as a Buffer) to check
   */
  addWarmedStorage(address: Buffer, slot: Buffer): void {
    const addressKey = address.toString('hex')
    let storageSet = this._accessedStorage[this._accessedStorage.length - 1].get(addressKey)
    if (!storageSet) {
      storageSet = new Set()
      this._accessedStorage[this._accessedStorage.length - 1].set(addressKey, storageSet!)
    }
    storageSet!.add(slot.toString('hex'))
  }

  /**
   * Clear the warm accounts and storage. To be called after a transaction finished.
   */
  clearWarmedAccounts(): void {
    this._accessedStorage = [new Map()]
    this._accessedStorageReverted = [new Map()]
  }

  /**
   * Generates an EIP-2930 access list
   *
   * Note: this method is not yet part of the {@link StateManager} interface.
   * If not implemented, {@link VM.runTx} is not allowed to be used with the
   * `reportAccessList` option and will instead throw.
   *
   * Note: there is an edge case on accessList generation where an
   * internal call might revert without an accessList but pass if the
   * accessList is used for a tx run (so the subsequent behavior might change).
   * This edge case is not covered by this implementation.
   *
   * @param addressesRemoved - List of addresses to be removed from the final list
   * @param addressesOnlyStorage - List of addresses only to be added in case of present storage slots
   *
   * @returns - an [@nomicfoundation/ethereumjs-tx](https://github.com/ethereumjs/ethereumjs-monorepo/packages/tx) `AccessList`
   */
  generateAccessList(
    addressesRemoved: Address[] = [],
    addressesOnlyStorage: Address[] = []
  ): AccessList {
    // Merge with the reverted storage list
    const mergedStorage = [...this._accessedStorage, ...this._accessedStorageReverted]

    // Fold merged storage array into one Map
    while (mergedStorage.length >= 2) {
      const storageMap = mergedStorage.pop()
      if (storageMap) {
        this._accessedStorageMerge(mergedStorage, storageMap)
      }
    }
    const folded = new Map([...mergedStorage[0].entries()].sort())

    // Transfer folded map to final structure
    const accessList: AccessList = []
    for (const [addressStr, slots] of folded.entries()) {
      const address = Address.fromString(`0x${addressStr}`)
      const check1 = addressesRemoved.find((a) => a.equals(address))
      const check2 =
        addressesOnlyStorage.find((a) => a.equals(address)) !== undefined && slots.size === 0

      if (!check1 && !check2) {
        const storageSlots = Array.from(slots)
          .map((s) => `0x${s}`)
          .sort()
        const accessListItem: AccessListItem = {
          address: `0x${addressStr}`,
          storageKeys: storageSlots,
        }
        accessList!.push(accessListItem)
      }
    }

    return accessList
  }

  /**
   * Checks if the `account` corresponding to `address`
   * is empty or non-existent as defined in
   * EIP-161 (https://eips.ethereum.org/EIPS/eip-161).
   * @param address - Address to check
   */
  async accountIsEmpty(address: Address): Promise<boolean> {
    return this._stateManager.accountIsEmpty(address)
  }
}
