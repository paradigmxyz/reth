import path from 'path';
import type { SolcOutput, SolcInput } from '../solc';
import type { SrcDecoder } from '../utils';

interface Source {
  name: string;
  content: Buffer;
}

export function srcDecoder(solcInput: SolcInput, solcOutput: SolcOutput, basePath = '.'): SrcDecoder {
  const sources: Record<number, Source> = {};

  function getSource(sourceId: number): Source {
    if (sourceId in sources) {
      return sources[sourceId];
    } else {
      const sourcePath = Object.entries(solcOutput.sources).find(([, { id }]) => sourceId === id)?.[0];
      if (sourcePath === undefined) {
        throw new Error(`Source file not available`);
      }
      const content = solcInput.sources[sourcePath]?.content;
      const name = path.relative(basePath, sourcePath);
      if (content === undefined) {
        throw new Error(`Content for '${name}' not available`);
      }
      return (sources[sourceId] = { name, content: Buffer.from(content, 'utf8') });
    }
  }

  return ({ src }) => {
    const [begin, , sourceId] = src.split(':').map(Number);
    const { name, content } = getSource(sourceId);
    const line = 1 + countInBuffer(content.subarray(0, begin), '\n');
    return name + ':' + line;
  };
}

function countInBuffer(buf: Buffer, str: string): number {
  let count = 0;
  let from = 0;
  while (true) {
    let i = buf.indexOf(str, from);
    if (i === -1) break;
    count += 1;
    from = i + str.length;
  }
  return count;
}
