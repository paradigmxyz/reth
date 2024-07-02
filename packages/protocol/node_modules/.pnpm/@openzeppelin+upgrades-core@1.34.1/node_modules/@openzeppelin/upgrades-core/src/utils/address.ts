import { toChecksumAddress } from 'ethereumjs-util';

/**
 * Parses an address from a hex string which may come from storage or a returned address via eth_call.
 *
 * @param addressString The address hex string.
 * @returns The parsed checksum address, or undefined if the input string is not an address.
 */
export function parseAddress(addressString: string): string | undefined {
  const buf = Buffer.from(addressString.replace(/^0x/, ''), 'hex');
  if (!buf.slice(0, 12).equals(Buffer.alloc(12, 0))) {
    return undefined;
  }
  const address = '0x' + buf.toString('hex', 12, 32); // grab the last 20 bytes
  return toChecksumAddress(address);
}
