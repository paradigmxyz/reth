import debug from './utils/debug';
import { callOptionalSignature } from './call-optional-signature';
import { EthereumProvider } from './provider';

export async function getUpgradeInterfaceVersion(
  provider: EthereumProvider,
  address: string,
  log = debug,
): Promise<string | undefined> {
  const encodedVersion = await callOptionalSignature(provider, address, 'UPGRADE_INTERFACE_VERSION()');
  if (encodedVersion !== undefined) {
    // Encoded string
    const buf = Buffer.from(encodedVersion.replace(/^0x/, ''), 'hex');

    // The first 32 bytes represent the offset, which should be 32 for a string
    const offset = parseInt(buf.slice(0, 32).toString('hex'), 16);
    if (offset !== 32) {
      // Log as debug and return undefined if the interface version is not a string.
      // Do not throw an error because this could be caused by a fallback function.
      log(`Unexpected type for UPGRADE_INTERFACE_VERSION at address ${address}. Expected a string`);
      return undefined;
    }

    // The next 32 bytes represent the length of the string
    const length = parseInt(buf.slice(32, 64).toString('hex'), 16);

    // The rest is the string itself
    return buf.slice(64, 64 + length).toString('utf8');
  } else {
    return undefined;
  }
}
