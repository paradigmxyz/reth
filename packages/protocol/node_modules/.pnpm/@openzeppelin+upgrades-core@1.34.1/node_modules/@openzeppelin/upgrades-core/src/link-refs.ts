import { SolcBytecode } from './solc-api';

export interface LinkReference {
  src: string;
  name: string;
  length: number;
  start: number;
  placeholder: string;
}

export function extractLinkReferences(bytecode: SolcBytecode): LinkReference[] {
  const linkRefs: LinkReference[] = [];
  const { linkReferences } = bytecode;
  for (const source of Object.keys(linkReferences)) {
    for (const name of Object.keys(linkReferences[source])) {
      for (const { length, start } of linkReferences[source][name]) {
        const placeholder = bytecode.object.substr(start * 2, length * 2);
        linkRefs.push({
          src: source,
          name,
          length,
          start,
          placeholder,
        });
      }
    }
  }
  return linkRefs;
}

export function unlinkBytecode(bytecode: string, linkReferences: LinkReference[]): string {
  let unlinkedBytecode: string = bytecode.replace(/^0x/, '');
  for (const linkRef of linkReferences) {
    const { length, start, placeholder } = linkRef;
    if ((start + length) * 2 <= unlinkedBytecode.length) {
      unlinkedBytecode =
        unlinkedBytecode.substr(0, start * 2) + placeholder + unlinkedBytecode.substr((start + length) * 2);
    }
  }
  return '0x' + unlinkedBytecode;
}
