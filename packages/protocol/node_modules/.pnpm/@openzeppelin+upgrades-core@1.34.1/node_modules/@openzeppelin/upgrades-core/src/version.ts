import { keccak256 } from 'ethereumjs-util';
import cbor from 'cbor';

export interface Version {
  withMetadata: string;
  withoutMetadata: string;
  linkedWithoutMetadata: string;
}

export function getVersion(bytecode: string, linkedBytecode?: string, constructorArgs = ''): Version {
  if (bytecode !== '') {
    return {
      withMetadata: hashBytecode(bytecode),
      withoutMetadata: hashBytecodeWithoutMetadata(bytecode),
      linkedWithoutMetadata: hashBytecodeWithoutMetadata(linkedBytecode ?? bytecode, constructorArgs),
    };
  } else {
    throw new Error('Abstract contract not allowed here');
  }
}

export function hashBytecode(bytecode: string, constructorArgs = ''): string {
  bytecode = bytecode
    .replace(/__\$([0-9a-fA-F]{34})\$__/g, (_, placeholder) => `000${placeholder}000`)
    .replace(/__\w{36}__/g, placeholder => keccak256(Buffer.from(placeholder)).toString('hex', 0, 20));

  // Fail if bytecode is still not valid hex after transformations above.
  // Buffer.from silently truncates at the first non-hex character.
  // NOTE: Some bytecode seems to have odd length, so we cannot do ([0-9a-fA-F]{2})*
  if (!/^(0x)?[0-9a-fA-F]*$/.test(bytecode)) {
    throw new Error('Bytecode is not a valid hex string');
  }

  if (!/^(0x)?[0-9a-fA-F]*$/.test(constructorArgs)) {
    throw new Error('Constructor arguments are not a valid hex string');
  }

  const buf = Buffer.concat([
    Buffer.from(bytecode.replace(/^0x/, ''), 'hex'),
    Buffer.from(constructorArgs.replace(/^0x/, ''), 'hex'),
  ]);

  return keccak256(buf).toString('hex');
}

export function hashBytecodeWithoutMetadata(bytecode: string, constructorArgs = ''): string {
  return hashBytecode(trimBytecodeMetadata(bytecode), constructorArgs);
}

function trimBytecodeMetadata(bytecode: string): string {
  // Bail on empty bytecode
  if (bytecode.length <= 4) {
    return bytecode;
  }

  // Gather length of CBOR metadata from the end of the file
  const rawLength = bytecode.slice(bytecode.length - 4);
  const metadataLength = parseInt(rawLength, 16) * 2;

  // Bail on unreasonable values for length
  if (metadataLength > bytecode.length - 4) {
    return bytecode;
  }

  // Gather what we assume is the CBOR encoded metadata, and try to parse it
  const metadataStart = bytecode.length - metadataLength - 4;
  const metadata = bytecode.slice(metadataStart, bytecode.length - 4);

  // Parse it to see if it is indeed valid metadata
  try {
    cbor.decode(Buffer.from(metadata, 'hex'));
  } catch (err) {
    // to do: log lack metadata to the user
    return bytecode;
  }

  // Return bytecode without it
  return bytecode.slice(0, metadataStart);
}
