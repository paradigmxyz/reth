import assert from 'assert';
import { ContractDefinition, StructDefinition } from 'solidity-ast';
import { isNodeType } from 'solidity-ast/utils';
import { StorageItem, StorageLayout, TypeItem, isStructMembers } from './layout';
import { SrcDecoder } from '../src-decoder';
import { getAnnotationArgs, getDocumentation, hasAnnotationTag } from '../utils/annotations';
import { Node } from 'solidity-ast/node';
import { CompilationContext, getTypeMembers, loadLayoutType } from './extract';
import { UpgradesError } from '../error';
import * as versions from 'compare-versions';

/**
 * Loads a contract's namespaces and namespaced type information into the storage layout.
 *
 * The provided compilation contexts must include both the original compilation and optionally
 * a namespaced compilation where contracts have been modified to include namespaced type information.
 *
 * If namespaced compilation is included, storage slots and offsets will be included in the loaded namespaces and types.
 *
 * This function looks up namespaces and their members from the namespaced compilation context's AST if available
 * (meaning node ids would be from the namespaced compilation), and looks up slots and offsets from the compiled type information.
 * However, it saves the original source locations from the original context so that line numbers are
 * consistent with the original source code.
 *
 * @param decodeSrc Source decoder for the original source code.
 * @param layout The storage layout object to load namespaces into.
 * @param origContext The original compilation context, which is used to lookup original source locations.
 * @param namespacedContext The namespaced compilation context, which represents a namespaced compilation.
 */
export function loadNamespaces(
  decodeSrc: SrcDecoder,
  layout: StorageLayout,
  origContext: CompilationContext,
  namespacedContext?: CompilationContext,
) {
  const namespacesWithSrc: Record<string, NamespaceWithSrc> = {};

  const origLinearized = origContext.contractDef.linearizedBaseContracts.map(id =>
    getReferencedContract(origContext, id),
  );
  const namespacedLinearized = namespacedContext?.contractDef.linearizedBaseContracts.map(id =>
    getReferencedContract(namespacedContext, id),
  );
  assert(namespacedLinearized === undefined || origLinearized.length === namespacedLinearized.length);
  const context = namespacedContext ?? origContext;
  const linearized = namespacedLinearized ?? origLinearized;
  for (const [i, contractDef] of linearized.entries()) {
    const origContractDef = origLinearized[i];
    const contractContext = { ...context, contractDef };
    addContractNamespacesWithSrc(
      namespacesWithSrc,
      decodeSrc,
      layout,
      contractContext,
      origContractDef,
      origContext.contractDef.canonicalName ?? origContext.contractDef.name,
    );
  }

  // Add to layout without the namespaced structs' src locations, since those are no longer needed
  // as they were only used to give duplicate namespace errors above.
  layout.namespaces = Object.fromEntries(
    Object.entries(namespacesWithSrc).map(([id, namespaceWithSrc]) => [id, namespaceWithSrc.namespace]),
  );
}

class DuplicateNamespaceError extends UpgradesError {
  constructor(id: string, contractName: string, src1: string, src2: string) {
    super(
      `Namespace ${id} is defined multiple times for contract ${contractName}`,
      () => `\
The namespace ${id} was found in structs at the following locations:
- ${src1}
- ${src2}

Use a unique namespace id for each struct annotated with '@custom:storage-location erc7201:<NAMESPACE_ID>' in your contract and its inherited contracts.`,
    );
  }
}

/**
 * Namespaced storage items, along with the original source location of the namespace struct.
 */
interface NamespaceWithSrc {
  namespace: StorageItem<string>[];
  src: string;
}

/**
 * Gets the contract definition for the given referenced id.
 */
function getReferencedContract(context: CompilationContext, referencedId: number) {
  // Optimization to avoid dereferencing if the referenced id is the same as the current contract
  return context.contractDef.id === referencedId
    ? context.contractDef
    : context.deref(['ContractDefinition'], referencedId);
}

/**
 * Add namespaces and source locations for the given compilation context's contract.
 * Does not include inherited contracts.
 *
 * @param namespacesWithSrc The record of namespaces with source locations to add to.
 * @param decodeSrc Source decoder for the original source code.
 * @param layout The storage layout object to load types into.
 * @param contractContext The compilation context for this specific contract to load namespaces for.
 * @param origContractDef The AST node for this specific contract but from the original compilation context.
 * @param leastDerivedContractName The name of the least derived contract in the inheritance list.
 * @throws DuplicateNamespaceError if a duplicate namespace is found when adding to the `namespaces` record.
 */
function addContractNamespacesWithSrc(
  namespacesWithSrc: Record<string, NamespaceWithSrc>,
  decodeSrc: SrcDecoder,
  layout: StorageLayout,
  contractContext: CompilationContext,
  origContractDef: ContractDefinition,
  leastDerivedContractName: string,
) {
  for (const node of contractContext.contractDef.nodes) {
    if (isNodeType('StructDefinition', node)) {
      const storageLocation = getStorageLocationAnnotation(node);
      if (storageLocation !== undefined) {
        const origSrc = decodeSrc(getOriginalStruct(node.canonicalName, origContractDef));

        if (namespacesWithSrc[storageLocation] !== undefined) {
          throw new DuplicateNamespaceError(
            storageLocation,
            leastDerivedContractName,
            namespacesWithSrc[storageLocation].src,
            origSrc,
          );
        } else {
          namespacesWithSrc[storageLocation] = {
            namespace: getNamespacedStorageItems(node, decodeSrc, layout, contractContext, origContractDef),
            src: origSrc,
          };
        }
      }
    }
  }
}

/**
 * Gets the storage location string from the `@custom:storage-location` annotation.
 *
 * For example, when using ERC-7201 (https://eips.ethereum.org/EIPS/eip-7201), the result will be `erc7201:<NAMESPACE_ID>`
 *
 * @param node The node that may have a `@custom:storage-location` annotation.
 * @returns The storage location string, or undefined if the node does not have a `@custom:storage-location` annotation.
 * @throws Error if the node has the annotation `@custom:storage-location` but it does not have exactly one argument.
 */
export function getStorageLocationAnnotation(node: Node) {
  const doc = getDocumentation(node);
  if (hasAnnotationTag(doc, 'storage-location')) {
    const storageLocationArgs = getAnnotationArgs(doc, 'storage-location');
    if (storageLocationArgs.length !== 1) {
      throw new Error('@custom:storage-location annotation must have exactly one argument');
    }
    return storageLocationArgs[0];
  }
}

/**
 * Gets the storage items for the given struct node.
 * Includes loading recursive type information, and adds slot and offset if they are available in the given compilation context's layout.
 */
function getNamespacedStorageItems(
  node: StructDefinition,
  decodeSrc: SrcDecoder,
  layout: StorageLayout,
  context: CompilationContext,
  origContractDef: ContractDefinition,
): StorageItem[] {
  const storageItems: StorageItem[] = [];

  for (const astMember of getTypeMembers(node, { typeName: true })) {
    const item: StorageItem = {
      contract: context.contractDef.name,
      label: astMember.label,
      type: astMember.type,
      src: decodeSrc({ src: getOriginalMemberSrc(node.canonicalName, astMember.label, origContractDef) }),
    };

    const layoutMember = findLayoutStructMember(
      context.storageLayout?.types ?? {},
      node.canonicalName,
      astMember.label,
    );

    if (layoutMember?.offset !== undefined && layoutMember?.slot !== undefined) {
      item.offset = layoutMember.offset;
      item.slot = layoutMember.slot;
    }

    storageItems.push(item);

    // If context is namespaced, we have storage layout, and this will fill in enum members just like in extractStorageLayout.
    // If context is original, this will add the types from the namespace structs to the layout.
    loadLayoutType(astMember.typeName, layout, context.deref);
  }
  return storageItems;
}

/**
 * Gets the struct definition matching the given canonical name from the original contract definition.
 */
function getOriginalStruct(structCanonicalName: string, origContractDef: ContractDefinition) {
  for (const node of origContractDef.nodes) {
    if (isNodeType('StructDefinition', node)) {
      if (node.canonicalName === structCanonicalName) {
        return node;
      }
    }
  }
  throw new Error(
    `Could not find original source location for namespace struct with name ${structCanonicalName} from contract ${origContractDef.name}`,
  );
}

/**
 * Gets the original source location for the given struct canonical name and struct member label.
 */
function getOriginalMemberSrc(structCanonicalName: string, memberLabel: string, origContractDef: ContractDefinition) {
  const node = getOriginalStruct(structCanonicalName, origContractDef);
  if (node !== undefined) {
    for (const member of getTypeMembers(node, { src: true })) {
      if (member.label === memberLabel) {
        return member.src;
      }
    }
  }

  throw new Error(
    `Could not find original source location for namespace struct with name ${structCanonicalName} and member ${memberLabel}`,
  );
}

/**
 * From the given layout types, gets the struct member matching the given struct canonical name and struct member label.
 */
function findLayoutStructMember(
  types: Record<string, TypeItem<string>>,
  structCanonicalName: string,
  memberLabel: string,
) {
  const structType = findTypeWithLabel(types, `struct ${structCanonicalName}`);
  const structMembers = structType?.members;
  if (structMembers !== undefined) {
    assert(isStructMembers(structMembers));
    for (const structMember of structMembers) {
      if (structMember.label === memberLabel) {
        return structMember;
      }
    }
  }
}

/**
 * From the given layout types, gets the type matching the given type label.
 */
function findTypeWithLabel(types: Record<string, TypeItem>, label: string) {
  return Object.values(types).find(type => type.label === label);
}

export function isNamespaceSupported(solcVersion: string) {
  return versions.compare(solcVersion, '0.8.20', '>=');
}
