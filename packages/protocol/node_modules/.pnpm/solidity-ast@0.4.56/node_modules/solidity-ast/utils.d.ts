import { SolcInput, SolcOutput } from './solc';
import { Node, NodeType, NodeTypeMap } from './node';
import { SourceUnit } from './types';

import { isNodeType, ExtendedNodeType, ExtendedNodeTypeMap } from './utils/is-node-type';
export { isNodeType, ExtendedNodeType, ExtendedNodeTypeMap };

export { findAll } from './utils/find-all';

export interface NodeWithSourceUnit<N extends Node = Node> {
  node: N;
  sourceUnit: SourceUnit;
}

export interface ASTDereferencer {
  <T extends ExtendedNodeType>(nodeType: T | readonly T[]): (id: number) => ExtendedNodeTypeMap[T];
  <T extends ExtendedNodeType>(nodeType: T | readonly T[], id: number): ExtendedNodeTypeMap[T];
  withSourceUnit<T extends ExtendedNodeType>(nodeType: T | readonly T[], id: number): NodeWithSourceUnit<ExtendedNodeTypeMap[T]>;
}

export function astDereferencer(solcOutput: SolcOutput): ASTDereferencer;

export class ASTDereferencerError extends Error {
  readonly id: number;
  readonly nodeType: readonly ExtendedNodeType[];
}

export type SrcDecoder = (node: { src: string }) => string;

export function srcDecoder(solcInput: SolcInput, solcOutput: SolcOutput): SrcDecoder;
