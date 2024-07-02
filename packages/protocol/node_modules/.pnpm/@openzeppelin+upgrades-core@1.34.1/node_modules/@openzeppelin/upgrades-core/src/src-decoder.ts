import path from 'path';
import { SolcOutput, SolcInput } from './solc-api';

export type SrcDecoder = (node: { src: string }) => string;

interface Source {
  name: string;
  content: string;
}

export function solcInputOutputDecoder(solcInput: SolcInput, solcOutput: SolcOutput, basePath = '.'): SrcDecoder {
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
        throw new Error(`Content for ${name} not available`);
      }
      return (sources[sourceId] = { name, content });
    }
  }

  return ({ src }) => {
    const [begin, , sourceId] = src.split(':').map(Number);
    const { name, content } = getSource(sourceId);
    const line = content.substr(0, begin).split('\n').length;
    return name + ':' + line;
  };
}
