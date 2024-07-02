import type { Proof } from './stateManager'
import type { Account, Address } from '@nomicfoundation/ethereumjs-util'

/**
 * Storage values of an account
 */
export interface StorageDump {
  [key: string]: string
}

export type AccountFields = Partial<Pick<Account, 'nonce' | 'balance' | 'storageRoot' | 'codeHash'>>

export interface StateAccess {
  accountExists(address: Address): Promise<boolean>
  getAccount(address: Address): Promise<Account>
  putAccount(address: Address, account: Account): Promise<void>
  accountIsEmpty(address: Address): Promise<boolean>
  deleteAccount(address: Address): Promise<void>
  modifyAccountFields(address: Address, accountFields: AccountFields): Promise<void>
  putContractCode(address: Address, value: Buffer): Promise<void>
  getContractCode(address: Address): Promise<Buffer>
  getContractStorage(address: Address, key: Buffer): Promise<Buffer>
  putContractStorage(address: Address, key: Buffer, value: Buffer): Promise<void>
  clearContractStorage(address: Address): Promise<void>
  checkpoint(): Promise<void>
  commit(): Promise<void>
  revert(): Promise<void>
  getStateRoot(): Promise<Buffer>
  setStateRoot(stateRoot: Buffer): Promise<void>
  getProof?(address: Address, storageSlots: Buffer[]): Promise<Proof>
  verifyProof?(proof: Proof): Promise<boolean>
  hasStateRoot(root: Buffer): Promise<boolean>
}

export interface StateManager extends StateAccess {
  copy(): StateManager
  flush(): Promise<void>
  dumpStorage(address: Address): Promise<StorageDump>
}
