import { addHexPrefix, bigIntToHex, isHexPrefixed } from '@nomicfoundation/ethereumjs-util'

import type { GenesisState } from './genesisStates'

/**
 * Parses the geth genesis state into Blockchain {@link GenesisState}
 * @param json representing the `alloc` key in a Geth genesis file
 */
export function parseGethGenesisState(json: any) {
  const state: GenesisState = {}
  for (let address of Object.keys(json.alloc)) {
    let { balance, code, storage } = json.alloc[address]
    address = addHexPrefix(address)
    balance = isHexPrefixed(balance) ? balance : bigIntToHex(BigInt(balance))
    code = code !== undefined ? addHexPrefix(code) : undefined
    storage = storage !== undefined ? Object.entries(storage) : undefined
    state[address] = [balance, code, storage] as any
  }
  return state
}
