import { Node, NodeType, NodeTypeMap } from '../node';

export type ExtendedNodeType = '*' | NodeType;

export interface ExtendedNodeTypeMap extends NodeTypeMap {
  '*': Node;
}

export function isNodeType<N extends Node, T extends ExtendedNodeType>(nodeType: T | readonly T[]): (node: N) => node is N & ExtendedNodeTypeMap[T];
export function isNodeType<N extends Node, T extends ExtendedNodeType>(nodeType: T | readonly T[], node: N): node is N & ExtendedNodeTypeMap[T];

