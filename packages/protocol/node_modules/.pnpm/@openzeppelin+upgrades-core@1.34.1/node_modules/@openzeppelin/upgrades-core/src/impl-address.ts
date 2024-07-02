import {
  EIP1967BeaconNotFound,
  EIP1967ImplementationNotFound,
  getBeaconAddress,
  getImplementationAddress,
  UpgradesError,
} from '.';
import { callOptionalSignature } from './call-optional-signature';

import { EthereumProvider } from './provider';
import { parseAddress } from './utils/address';

export class InvalidBeacon extends UpgradesError {}

/**
 * Gets the implementation address from the beacon using its implementation() function.
 * @param provider
 * @param beaconAddress
 * @returns The implementation address.
 * @throws {InvalidBeacon} If the implementation() function could not be called or does not return an address.
 */
export async function getImplementationAddressFromBeacon(
  provider: EthereumProvider,
  beaconAddress: string,
): Promise<string> {
  const impl = await callOptionalSignature(provider, beaconAddress, 'implementation()');
  let parsedImplAddress;
  if (impl !== undefined) {
    parsedImplAddress = parseAddress(impl);
  }

  if (parsedImplAddress === undefined) {
    throw new InvalidBeacon(`Contract at ${beaconAddress} doesn't look like a beacon`);
  } else {
    return parsedImplAddress;
  }
}

/**
 * Gets the implementation address from a UUPS/Transparent/Beacon proxy.
 *
 * @returns a Promise with the implementation address, or undefined if a UUPS/Transparent/Beacon proxy is not located at the address.
 */
export async function getImplementationAddressFromProxy(
  provider: EthereumProvider,
  proxyAddress: string,
): Promise<string | undefined> {
  try {
    return await getImplementationAddress(provider, proxyAddress);
  } catch (e: any) {
    if (e instanceof EIP1967ImplementationNotFound) {
      try {
        const beaconAddress = await getBeaconAddress(provider, proxyAddress);
        return await getImplementationAddressFromBeacon(provider, beaconAddress);
      } catch (e: any) {
        if (e instanceof EIP1967BeaconNotFound) {
          return undefined;
        } else {
          throw e;
        }
      }
    } else {
      throw e;
    }
  }
}
