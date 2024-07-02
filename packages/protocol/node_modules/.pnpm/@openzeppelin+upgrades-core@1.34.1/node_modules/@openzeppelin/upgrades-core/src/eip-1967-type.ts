import {
  EIP1967BeaconNotFound,
  EIP1967ImplementationNotFound,
  getAdminAddress,
  getBeaconAddress,
  getImplementationAddress,
  isEmptySlot,
} from './eip-1967';
import { EthereumProvider } from './provider';

export async function isTransparentOrUUPSProxy(provider: EthereumProvider, address: string): Promise<boolean> {
  try {
    await getImplementationAddress(provider, address);
    // if an exception was not encountered above, then this address is a transparent/uups proxy
    return true;
  } catch (e: any) {
    if (e instanceof EIP1967ImplementationNotFound) {
      return false;
    } else {
      throw e;
    }
  }
}

export async function isTransparentProxy(provider: EthereumProvider, address: string): Promise<boolean> {
  const adminAddress = await getAdminAddress(provider, address);
  return !isEmptySlot(adminAddress);
}

export async function isBeaconProxy(provider: EthereumProvider, address: string): Promise<boolean> {
  try {
    await getBeaconAddress(provider, address);
    // if an exception was not encountered above, then this address is a beacon proxy
    return true;
  } catch (e: any) {
    if (e instanceof EIP1967BeaconNotFound) {
      return false;
    } else {
      throw e;
    }
  }
}
