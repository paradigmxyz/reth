import { SourceUnit } from './types';

export interface SolcOutput {
  sources: {
    [file in string]: {
      ast: SourceUnit;
      id: number;
    };
  };
}

export interface SolcInput {
  sources: {
    [file in string]: {
      keccak256?: string;
      content?: string;
      urls?: string[];
    };
  };
}
