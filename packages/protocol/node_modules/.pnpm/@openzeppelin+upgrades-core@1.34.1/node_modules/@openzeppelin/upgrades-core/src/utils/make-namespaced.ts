import { isNodeType } from 'solidity-ast/utils';
import { Node } from 'solidity-ast/node';
import { SolcInput, SolcOutput } from '../solc-api';
import { getStorageLocationAnnotation } from '../storage/namespace';
import { assert } from './assert';

const OUTPUT_SELECTION = {
  '*': {
    '*': ['storageLayout'],
    '': ['ast'],
  },
};

/**
 * Makes a modified version of the solc input to add state variables in each contract for namespaced struct definitions,
 * so that the compiler will generate their types in the storage layout.
 *
 * This makes the following modifications to the input:
 * - Adds a state variable for each namespaced struct definition
 * - For each contract, for all node types that are not needed for storage layout or may reference deleted functions and constructors, converts them to dummy enums with random id
 * - Converts all using for directives (at file level and in contracts) to dummy enums with random id (do not delete them to avoid orphaning possible NatSpec documentation)
 * - Converts all custom errors, free functions and constants (at file level) to dummy enums with the same name (do not delete them since they might be imported by other files)
 *
 * Also sets the outputSelection to only include storageLayout and ast, since the other outputs are not needed.
 *
 * @param input The original solc input.
 * @param output The original solc output.
 * @returns The modified solc input with storage layout that includes namespaced type information.
 */
export function makeNamespacedInput(input: SolcInput, output: SolcOutput): SolcInput {
  const modifiedSources: Record<string, { content?: string }> = {};

  for (const [sourcePath] of Object.entries(input.sources)) {
    const source = input.sources[sourcePath];

    if (source.content === undefined) {
      modifiedSources[sourcePath] = source;
      continue;
    }

    const orig = Buffer.from(source.content, 'utf8');

    const replacedIdentifiers = new Set<string>();
    const modifications: Modification[] = [];

    for (const node of output.sources[sourcePath].ast.nodes) {
      switch (node.nodeType) {
        case 'ContractDefinition': {
          const contractDef = node;

          // Remove any calls to parent constructors from the inheritance list
          const inherits = contractDef.baseContracts;
          for (const inherit of inherits) {
            if (Array.isArray(inherit.arguments)) {
              assert(inherit.baseName.name !== undefined);
              modifications.push(makeReplace(inherit, orig, inherit.baseName.name));
            }
          }

          const contractNodes = contractDef.nodes;
          for (const contractNode of contractNodes) {
            switch (contractNode.nodeType) {
              case 'VariableDeclaration': {
                // If variable is a constant, keep it since it may be referenced in a struct
                if (contractNode.constant) {
                  break;
                }
                // Otherwise, fall through to convert to dummy enum
              }
              case 'ErrorDefinition':
              case 'EventDefinition':
              case 'FunctionDefinition':
              case 'ModifierDefinition':
              case 'UsingForDirective': {
                // Replace with an enum based on astId (the original name is not needed, since nothing should reference it)
                modifications.push(makeReplace(contractNode, orig, toDummyEnumWithAstId(contractNode.id)));
                break;
              }
              case 'StructDefinition': {
                const storageLocation = getStorageLocationAnnotation(contractNode);
                if (storageLocation !== undefined) {
                  const structName = contractNode.name;
                  const variableName = `$${structName}_${(Math.random() * 1e6).toFixed(0)}`;
                  const insertText = ` ${structName} ${variableName};`;

                  modifications.push(makeInsertAfter(contractNode, insertText));
                }
                break;
              }
              case 'EnumDefinition':
              case 'UserDefinedValueTypeDefinition':
              default: {
                // - EnumDefinition may be used in structures with storage locations
                // - UserDefinedValueTypeDefinition may be used in structures with storage locations
                // - default: in case unexpected ast nodes show up
                break;
              }
            }
          }
          break;
        }

        // - UsingForDirective isn't needed, but it might have NatSpec documentation which is not included in the AST.
        //   We convert it to a dummy enum to avoid orphaning any possible documentation.
        // - ErrorDefinition, FunctionDefinition, and VariableDeclaration might be imported by other files, so they cannot be deleted.
        //   However, we need to remove their values to avoid referencing other deleted nodes.
        //   We do this by converting them to dummy enums, but avoiding duplicate names.
        case 'UsingForDirective':
        case 'ErrorDefinition':
        case 'FunctionDefinition':
        case 'VariableDeclaration': {
          // If an identifier with the same name was not previously written, replace with a dummy enum using its name.
          // Otherwise replace with an enum based on astId to avoid duplicate names, which can happen if there was overloading.
          // This does not need to check all identifiers from the original contract, since the original compilation
          // should have failed if there were conflicts in the first place.
          if ('name' in node && !replacedIdentifiers.has(node.name)) {
            modifications.push(makeReplace(node, orig, toDummyEnumWithName(node.name)));
            replacedIdentifiers.add(node.name);
          } else {
            modifications.push(makeReplace(node, orig, toDummyEnumWithAstId(node.id)));
          }
          break;
        }
        case 'EnumDefinition':
        case 'ImportDirective':
        case 'PragmaDirective':
        case 'StructDefinition':
        case 'UserDefinedValueTypeDefinition':
        default: {
          // - EnumDefinition may be used in structures with storage locations
          // - ImportDirective may import types used in structures with storage locations
          // - PragmaDirective is necessary for compilation
          // - StructDefinition may be used in structures with storage locations
          // - UserDefinedValueTypeDefinition may be used in structures with storage locations
          // - default: in case unexpected ast nodes show up (file-level events since 0.8.22)
          break;
        }
      }
    }

    modifiedSources[sourcePath] = { ...source, content: getModifiedSource(orig, modifications) };
  }

  return { ...input, sources: modifiedSources, settings: { ...input.settings, outputSelection: OUTPUT_SELECTION } };
}

interface Modification {
  start: number;
  end: number;
  text?: string;
}

function toDummyEnumWithName(name: string) {
  return `enum ${name} { dummy }`;
}

function toDummyEnumWithAstId(astId: number) {
  return `enum $astId_${astId}_${(Math.random() * 1e6).toFixed(0)} { dummy }`;
}

function getPositions(node: Node) {
  const [start, length] = node.src.split(':').map(Number);
  const end = start + length;
  return { start, end };
}

function makeReplace(node: Node, orig: Buffer, text: string): Modification {
  // Replace is a delete and insert
  const { start, end } = makeDelete(node, orig);
  return { start, end, text };
}

function makeInsertAfter(node: Node, text: string): Modification {
  const { end } = getPositions(node);
  return { start: end, end, text };
}

function makeDelete(node: Node, orig: Buffer): Modification {
  const positions = getPositions(node);
  let end = positions.end;
  // For variables, skip past whitespaces and the first semicolon
  if (isNodeType('VariableDeclaration', node)) {
    while (end + 1 < orig.length && orig.toString('utf8', end, end + 1).trim() === '') {
      end += 1;
    }
    if (end + 1 < orig.length && orig.toString('utf8', end, end + 1) === ';') {
      end += 1;
    }
  }
  return { start: positions.start, end };
}

function getModifiedSource(orig: Buffer, modifications: Modification[]): string {
  let result = '';
  let copyFromIndex = 0;

  for (const modification of modifications) {
    assert(modification.start >= copyFromIndex);
    result += orig.toString('utf8', copyFromIndex, modification.start);

    if (modification.text !== undefined) {
      result += modification.text;
    }

    copyFromIndex = modification.end;
  }

  assert(copyFromIndex <= orig.length);
  result += orig.toString('utf8', copyFromIndex);

  return result;
}
