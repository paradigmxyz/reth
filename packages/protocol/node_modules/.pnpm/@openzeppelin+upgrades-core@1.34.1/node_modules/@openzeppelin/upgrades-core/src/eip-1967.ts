import { keccak256 } from 'ethereumjs-util';
import { UpgradesError } from './error';
import { EthereumProvider, getStorageAt } from './provider';
import { parseAddress } from './utils/address';

export class EIP1967ImplementationNotFound extends UpgradesError {}
export class EIP1967BeaconNotFound extends UpgradesError {}

export async function getAdminAddress(provider: EthereumProvider, address: string): Promise<string> {
  const storage = await getStorageFallback(
    provider,
    address,
    toEip1967Hash('eip1967.proxy.admin'),
    toFallbackEip1967Hash('org.zeppelinos.proxy.admin'),
  );

  return parseAddressFromStorage(storage);
}

export async function getImplementationAddress(provider: EthereumProvider, address: string): Promise<string> {
  const storage = await getStorageFallback(
    provider,
    address,
    toEip1967Hash('eip1967.proxy.implementation'),
    toFallbackEip1967Hash('org.zeppelinos.proxy.implementation'),
  );

  if (isEmptySlot(storage)) {
    throw new EIP1967ImplementationNotFound(
      `Contract at ${address} doesn't look like an ERC 1967 proxy with a logic contract address`,
    );
  }

  return parseAddressFromStorage(storage);
}

export async function getBeaconAddress(provider: EthereumProvider, address: string): Promise<string> {
  const storage = await getStorageFallback(provider, address, toEip1967Hash('eip1967.proxy.beacon'));

  if (isEmptySlot(storage)) {
    throw new EIP1967BeaconNotFound(`Contract at ${address} doesn't look like an ERC 1967 beacon proxy`);
  }

  return parseAddressFromStorage(storage);
}

async function getStorageFallback(provider: EthereumProvider, address: string, ...slots: string[]): Promise<string> {
  let storage = '0x0000000000000000000000000000000000000000000000000000000000000000'; // default: empty slot

  for (const slot of slots) {
    storage = await getStorageAt(provider, address, slot);
    if (!isEmptySlot(storage)) {
      break;
    }
  }

  return storage;
}

export function toFallbackEip1967Hash(label: string): string {
  return '0x' + keccak256(Buffer.from(label)).toString('hex');
}

export function toEip1967Hash(label: string): string {
  const hash = keccak256(Buffer.from(label));
  const bigNumber = BigInt('0x' + hash.toString('hex')) - 1n;
  return '0x' + bigNumber.toString(16);
}

export function isEmptySlot(storage: string): boolean {
  return BigInt(storage.replace(/^(0x)?/, '0x')) === 0n;
}

function parseAddressFromStorage(storage: string): string {
  const address = parseAddress(storage);
  if (address === undefined) {
    throw new Error(`Value in storage is not an address (${storage})`);
  }
  return address;
}
