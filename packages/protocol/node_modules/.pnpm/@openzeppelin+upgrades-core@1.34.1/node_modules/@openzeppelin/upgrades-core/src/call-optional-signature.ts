import { keccak256 } from 'ethereumjs-util';
import { call, EthereumProvider } from './provider';

export async function callOptionalSignature(provider: EthereumProvider, address: string, signature: string) {
  const data = '0x' + keccak256(Buffer.from(signature)).toString('hex').slice(0, 8);
  try {
    return await call(provider, address, data);
  } catch (e: any) {
    if (
      e.message.includes('function selector was not recognized') ||
      e.message.includes('invalid opcode') ||
      e.message.includes('revert') ||
      e.message.includes('execution error')
    ) {
      return undefined;
    } else {
      throw e;
    }
  }
}
