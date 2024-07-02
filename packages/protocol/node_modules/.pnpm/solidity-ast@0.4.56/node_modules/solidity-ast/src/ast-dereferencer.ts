import { isNodeType, ExtendedNodeType, ExtendedNodeTypeMap } from '../utils/is-node-type';
import { findAll } from '../utils/find-all';
import type { ASTDereferencer, NodeWithSourceUnit } from '../utils';
import type { Node, NodeType, NodeTypeMap } from '../node';
import type { SolcOutput } from '../solc';
import { SourceUnit } from '../types';

import findLast from 'array.prototype.findlast';

// An ASTDereferencer is a function that looks up an AST node given its id, in all of the source files involved in a
// solc run. It will generally be used together with the AST property `referencedDeclaration` (found in Identifier,
// UserDefinedTypeName, etc.) to look up a variable definition or type definition.

export function astDereferencer(solcOutput: SolcOutput): ASTDereferencer {
  const cache = new Map<number, NodeWithSourceUnit>();

  const asts = Array.from(
    Object.values(solcOutput.sources),
    s => s.ast,
  ).sort((a, b) => a.id - b.id);

  function deref<T extends ExtendedNodeType>(nodeType: T | readonly T[], id: number): NodeWithSourceUnit<ExtendedNodeTypeMap[T]>;
  function deref(nodeType: ExtendedNodeType | readonly ExtendedNodeType[], id: number): NodeWithSourceUnit {
    const cached = cache.get(id);

    if (cached) {
      if (isNodeType(nodeType, cached.node)) {
        return cached;
      }
    } else if (id >= 0) {
      // Node ids appear to be assigned in postorder. This means that a node's own id is always
      // larger than that of the nodes under it. We descend through the AST guided by this
      // assumption. Among a set of sibling nodes we choose the one with the smallest id that
      // is larger than the id we're looking for.

      let ast = asts.find(ast => (id <= ast.id));
      let searchRoot: Node | Node[] | undefined = ast;

      while (searchRoot) {
        if (Array.isArray(searchRoot)) {
          searchRoot = searchRoot.find(child => child && (id <= child.id));
        } else if (searchRoot.id === id) {
          break;
        } else {
          let nextRoot, nextRootId;
          for (const child of Object.values(searchRoot)) {
            if (typeof child !== "object") continue;
            const childId = Array.isArray(child) ? findLast(child, n => n)?.id : child?.id;
            if (childId === undefined) continue;
            if (id <= childId && (nextRootId === undefined || childId <= nextRootId)) {
              nextRoot = child;
              nextRootId = childId;
            }
          }
          searchRoot = nextRoot;
        }
      }

      let found: Node | undefined = searchRoot;

      // As a fallback mechanism in case the postorder assumption breaks, if the node is not found
      // we proceed to check all nodes in all ASTs.

      if (found === undefined) {
        outer: for (ast of asts) {
          for (const node of findAll(nodeType, ast)) {
            if (node.id === id) {
              found = node;
              break outer;
            }
          }
        }
      }

      if (found !== undefined) {
        const nodeWithSourceUnit = { node: found, sourceUnit: ast! };
        cache.set(id, nodeWithSourceUnit);
        if (isNodeType(nodeType, found)) {
          return nodeWithSourceUnit;
        }
      }
    }

    nodeType = Array.isArray(nodeType) ? nodeType : [nodeType];
    throw new ASTDereferencerError(id, nodeType);
  }

  function derefNode(nodeType: NodeType | readonly NodeType[], id: number) {
    return deref(nodeType, id).node;
  }

  return Object.assign(
    curry2(derefNode),
    { withSourceUnit: deref }
  );
}

export interface Curried<A, B, T> {
  (a: A): (b: B) => T;
  (a: A, b: B): T;
}

export function curry2<A, B, T>(fn: (a: A, b: B) => T): Curried<A, B, T> {
  function curried(a: A): (b: B) => T;
  function curried(a: A, b: B): T;
  function curried(a: A, ...b: [] | [B]): T | ((b: B) => T) {
    if (b.length === 0) {
      return b => fn(a, b);
    } else {
      return fn(a, ...b);
    }
  }
  return curried;
}

const isArray: (arg: any) => arg is any[] | readonly any[]  = Array.isArray;

export class ASTDereferencerError extends Error {
  constructor(readonly id: number, readonly nodeType: readonly ExtendedNodeType[]) {
    super(`No node with id ${id} of type ${nodeType}`);
  }
}
