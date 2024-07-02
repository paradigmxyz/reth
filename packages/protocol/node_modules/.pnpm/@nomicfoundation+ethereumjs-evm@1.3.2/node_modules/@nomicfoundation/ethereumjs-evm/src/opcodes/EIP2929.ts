import type { RunState } from '../interpreter'
import type { Common } from '@nomicfoundation/ethereumjs-common'
import type { Address } from '@nomicfoundation/ethereumjs-util'

/**
 * Adds address to accessedAddresses set if not already included.
 * Adjusts cost incurred for executing opcode based on whether address read
 * is warm/cold. (EIP 2929)
 * @param {RunState} runState
 * @param {Address}  address
 * @param {Common}   common
 * @param {Boolean}  chargeGas (default: true)
 * @param {Boolean}  isSelfdestruct (default: false)
 */
export function accessAddressEIP2929(
  runState: RunState,
  address: Address,
  common: Common,
  chargeGas = true,
  isSelfdestructOrAuthcall = false
): bigint {
  if (common.isActivatedEIP(2929) === false) return BigInt(0)

  const eei = runState.eei
  const addressStr = address.buf

  // Cold
  if (!eei.isWarmedAddress(addressStr)) {
    eei.addWarmedAddress(addressStr)

    // CREATE, CREATE2 opcodes have the address warmed for free.
    // selfdestruct beneficiary address reads are charged an *additional* cold access
    if (chargeGas) {
      return common.param('gasPrices', 'coldaccountaccess')
    }
    // Warm: (selfdestruct beneficiary address reads are not charged when warm)
  } else if (chargeGas && !isSelfdestructOrAuthcall) {
    return common.param('gasPrices', 'warmstorageread')
  }
  return BigInt(0)
}

/**
 * Adds (address, key) to accessedStorage tuple set if not already included.
 * Adjusts cost incurred for executing opcode based on whether storage read
 * is warm/cold. (EIP 2929)
 * @param {RunState} runState
 * @param {Buffer} key (to storage slot)
 * @param {Common} common
 */
export function accessStorageEIP2929(
  runState: RunState,
  key: Buffer,
  isSstore: boolean,
  common: Common
): bigint {
  if (common.isActivatedEIP(2929) === false) return BigInt(0)

  const eei = runState.eei
  const address = runState.interpreter.getAddress().buf
  const slotIsCold = !eei.isWarmedStorage(address, key)

  // Cold (SLOAD and SSTORE)
  if (slotIsCold) {
    eei.addWarmedStorage(address, key)
    return common.param('gasPrices', 'coldsload')
  } else if (!isSstore) {
    return common.param('gasPrices', 'warmstorageread')
  }
  return BigInt(0)
}

/**
 * Adjusts cost of SSTORE_RESET_GAS or SLOAD (aka sstorenoop) (EIP-2200) downward when storage
 * location is already warm
 * @param  {RunState} runState
 * @param  {Buffer}   key          storage slot
 * @param  {BigInt}   defaultCost  SSTORE_RESET_GAS / SLOAD
 * @param  {string}   costName     parameter name ('noop')
 * @param  {Common}   common
 * @return {BigInt}                adjusted cost
 */
export function adjustSstoreGasEIP2929(
  runState: RunState,
  key: Buffer,
  defaultCost: bigint,
  costName: string,
  common: Common
): bigint {
  if (common.isActivatedEIP(2929) === false) return defaultCost

  const eei = runState.eei
  const address = runState.interpreter.getAddress().buf
  const warmRead = common.param('gasPrices', 'warmstorageread')
  const coldSload = common.param('gasPrices', 'coldsload')

  if (eei.isWarmedStorage(address, key)) {
    switch (costName) {
      case 'noop':
        return warmRead
      case 'initRefund':
        return common.param('gasPrices', 'sstoreInitGasEIP2200') - warmRead
      case 'cleanRefund':
        return common.param('gasPrices', 'sstoreReset') - coldSload - warmRead
    }
  }

  return defaultCost
}
