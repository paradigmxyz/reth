import { Node, YulNode, YulNodeType, YulNodeTypeMap } from '../node';
import { ExtendedNodeType, ExtendedNodeTypeMap } from './is-node-type';

export function findAll<T extends ExtendedNodeType>(nodeType: T | readonly T[]): (node: Node) => Generator<ExtendedNodeTypeMap[T]>;
export function findAll<T extends ExtendedNodeType>(nodeType: T | readonly T[], node: Node, prune?: (node: Node) => boolean): Generator<ExtendedNodeTypeMap[T]>;

export function findAll<T extends ExtendedNodeType | YulNodeType>(nodeType: T | readonly T[]): (node: Node | YulNode) => Generator<(ExtendedNodeTypeMap & YulNodeTypeMap)[T]>;
export function findAll<T extends ExtendedNodeType | YulNodeType>(nodeType: T | readonly T[], node: Node | YulNode, prune?: (node: Node | YulNode) => boolean): Generator<(ExtendedNodeTypeMap & YulNodeTypeMap)[T]>;

