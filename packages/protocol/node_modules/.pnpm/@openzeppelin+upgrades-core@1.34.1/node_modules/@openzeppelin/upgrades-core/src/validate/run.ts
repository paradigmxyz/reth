import { Node } from 'solidity-ast/node';
import { isNodeType, findAll, ASTDereferencer, astDereferencer } from 'solidity-ast/utils';
import type {
  ContractDefinition,
  FunctionDefinition,
  StructDefinition,
  TypeName,
  UserDefinedTypeName,
  VariableDeclaration,
} from 'solidity-ast';
import debug from '../utils/debug';

import { SolcOutput, SolcBytecode, SolcInput } from '../solc-api';
import { SrcDecoder } from '../src-decoder';
import { isNullish } from '../utils/is-nullish';
import { getFunctionSignature } from '../utils/function';
import { Version, getVersion } from '../version';
import { extractLinkReferences, LinkReference } from '../link-refs';
import { extractStorageLayout } from '../storage/extract';
import { StorageLayout } from '../storage/layout';
import { getFullyQualifiedName } from '../utils/contract-name';
import { getAnnotationArgs as getSupportedAnnotationArgs, getDocumentation } from '../utils/annotations';
import { getStorageLocationAnnotation, isNamespaceSupported } from '../storage/namespace';
import { UpgradesError } from '../error';
import { assertUnreachable } from '../utils/assert';

export type ValidationRunData = Record<string, ContractValidation>;

export interface ContractValidation {
  version?: Version;
  src: string;
  inherit: string[];
  libraries: string[];
  methods: string[];
  linkReferences: LinkReference[];
  errors: ValidationError[];
  layout: StorageLayout;
  solcVersion?: string;
}

export const errorKinds = [
  'state-variable-assignment',
  'state-variable-immutable',
  'external-library-linking',
  'struct-definition',
  'enum-definition',
  'constructor',
  'delegatecall',
  'selfdestruct',
  'missing-public-upgradeto',
  'internal-function-storage',
] as const;

export type ValidationError =
  | ValidationErrorConstructor
  | ValidationErrorOpcode
  | ValidationErrorWithName
  | ValidationErrorUpgradeability;

interface ValidationErrorBase {
  src: string;
  kind: (typeof errorKinds)[number];
}

interface ValidationErrorWithName extends ValidationErrorBase {
  name: string;
  kind:
    | 'state-variable-assignment'
    | 'state-variable-immutable'
    | 'external-library-linking'
    | 'struct-definition'
    | 'enum-definition'
    | 'internal-function-storage';
}

interface ValidationErrorConstructor extends ValidationErrorBase {
  kind: 'constructor';
  contract: string;
}

interface ValidationErrorOpcode extends ValidationErrorBase {
  kind: 'delegatecall' | 'selfdestruct';
}

interface OpcodePattern {
  kind: 'delegatecall' | 'selfdestruct';
  pattern: RegExp;
}

const OPCODES = {
  delegatecall: {
    kind: 'delegatecall',
    pattern: /^t_function_baredelegatecall_/,
  },
  selfdestruct: {
    kind: 'selfdestruct',
    pattern: /^t_function_selfdestruct_/,
  },
} as const;

export function isOpcodeError(error: ValidationErrorBase): error is ValidationErrorOpcode {
  return error.kind === 'delegatecall' || error.kind === 'selfdestruct';
}

interface ValidationErrorUpgradeability extends ValidationErrorBase {
  kind: 'missing-public-upgradeto';
}

function getAllowed(node: Node, reachable: boolean): string[] {
  if ('documentation' in node) {
    const tag = `oz-upgrades-unsafe-allow${reachable ? '-reachable' : ''}`;
    const doc = getDocumentation(node);
    return getAnnotationArgs(doc, tag);
  } else {
    return [];
  }
}

export function getAnnotationArgs(doc: string, tag: string) {
  return getSupportedAnnotationArgs(doc, tag, errorKinds);
}

function skipCheckReachable(error: string, node: Node): boolean {
  return getAllowed(node, true).includes(error);
}

function skipCheck(error: string, node: Node): boolean {
  // skip both allow and allow-reachable errors in the lexical scope
  return getAllowed(node, false).includes(error) || getAllowed(node, true).includes(error);
}

/**
 * Runs validations on the given solc output.
 *
 * If `namespacedOutput` is provided, it is used to extract storage layout information for namespaced types.
 * It must be from a compilation with the same sources as `solcInput` and `solcOutput`, but with storage variables
 * injected for each namespaced struct so that the types are available in the storage layout. This can be obtained by
 * calling the `makeNamespacedInput` function from this package to create modified solc input, then compiling
 * that modified solc input to get the namespaced output.
 *
 * @param solcOutput Solc output to validate
 * @param decodeSrc Source decoder for the original source code
 * @param solcVersion The version of solc used to compile the contracts
 * @param solcInput Solc input that the compiler was invoked with
 * @param namespacedOutput Namespaced solc output to extract storage layout information for namespaced types
 * @returns A record of validation results for each fully qualified contract name
 */
export function validate(
  solcOutput: SolcOutput,
  decodeSrc: SrcDecoder,
  solcVersion?: string,
  solcInput?: SolcInput,
  namespacedOutput?: SolcOutput,
): ValidationRunData {
  const validation: ValidationRunData = {};
  const fromId: Record<number, string> = {};
  const inheritIds: Record<string, number[]> = {};
  const libraryIds: Record<string, number[]> = {};

  const deref = astDereferencer(solcOutput);

  const delegateCallCache = initOpcodeCache();
  const selfDestructCache = initOpcodeCache();

  for (const source in solcOutput.contracts) {
    checkNamespaceSolidityVersion(source, solcVersion, solcInput);
    checkNamespacesOutsideContract(source, solcOutput, decodeSrc);

    for (const contractName in solcOutput.contracts[source]) {
      const bytecode = solcOutput.contracts[source][contractName].evm.bytecode;
      const version = bytecode.object === '' ? undefined : getVersion(bytecode.object);
      const linkReferences = extractLinkReferences(bytecode);

      validation[getFullyQualifiedName(source, contractName)] = {
        src: contractName,
        version,
        inherit: [],
        libraries: [],
        methods: [],
        linkReferences,
        errors: [],
        layout: {
          storage: [],
          types: {},
        },
        solcVersion,
      };
    }

    for (const contractDef of findAll('ContractDefinition', solcOutput.sources[source].ast)) {
      const key = getFullyQualifiedName(source, contractDef.name);

      fromId[contractDef.id] = key;

      // May be undefined in case of duplicate contract names in Truffle
      const bytecode = solcOutput.contracts[source][contractDef.name]?.evm.bytecode;

      if (key in validation && bytecode !== undefined) {
        inheritIds[key] = contractDef.linearizedBaseContracts.slice(1);
        libraryIds[key] = getReferencedLibraryIds(contractDef);

        validation[key].src = decodeSrc(contractDef);
        validation[key].errors = [
          ...getConstructorErrors(contractDef, decodeSrc),
          ...getOpcodeErrors(contractDef, deref, decodeSrc, delegateCallCache, selfDestructCache),
          ...getStateVariableErrors(contractDef, decodeSrc),
          // TODO: add linked libraries support
          // https://github.com/OpenZeppelin/openzeppelin-upgrades/issues/52
          ...getLinkingErrors(contractDef, bytecode),
          ...getInternalFunctionStorageErrors(contractDef, deref, decodeSrc),
        ];

        validation[key].layout = extractStorageLayout(
          contractDef,
          decodeSrc,
          deref,
          solcOutput.contracts[source][contractDef.name].storageLayout,
          getNamespacedCompilationContext(source, contractDef, namespacedOutput),
        );

        validation[key].methods = [...findAll('FunctionDefinition', contractDef)]
          .filter(fnDef => ['external', 'public'].includes(fnDef.visibility))
          .map(fnDef => getFunctionSignature(fnDef, deref));
      }
    }
  }

  for (const key in inheritIds) {
    validation[key].inherit = inheritIds[key].map(id => fromId[id]);
  }

  for (const key in libraryIds) {
    validation[key].libraries = libraryIds[key].map(id => fromId[id]);
  }

  return validation;
}

function checkNamespaceSolidityVersion(source: string, solcVersion?: string, solcInput?: SolcInput) {
  if (solcInput === undefined) {
    // This should only be missing if using an old version of the Hardhat or Truffle plugin.
    // Even without this param, namespace layout checks would still occur if compiled with solc version >= 0.8.20
    debug('Cannot check Solidity version for namespaces because solcInput is undefined');
  } else {
    // Solc versions older than 0.8.20 do not have documentation for structs.
    // Use a regex to check for strings that look like namespace annotations, and if found, check that the compiler version is >= 0.8.20
    const content = solcInput.sources[source].content;
    const hasMatch = content !== undefined && content.match(/@custom:storage-location/);
    if (hasMatch) {
      if (solcVersion === undefined) {
        throw new UpgradesError(
          `${source}: Namespace annotations require Solidity version >= 0.8.20, but no solcVersion parameter was provided`,
          () =>
            `Structs with the @custom:storage-location annotation can only be used with Solidity version 0.8.20 or higher. Pass the solcVersion parameter to the validate function, or remove the annotation if the struct is not used for namespaced storage.`,
        );
      } else if (!isNamespaceSupported(solcVersion)) {
        throw new UpgradesError(
          `${source}: Namespace annotations require Solidity version >= 0.8.20, but ${solcVersion} was used`,
          () =>
            `Structs with the @custom:storage-location annotation can only be used with Solidity version 0.8.20 or higher. Use a newer version of Solidity, or remove the annotation if the struct is not used for namespaced storage.`,
        );
      }
    }
  }
}

function checkNamespacesOutsideContract(source: string, solcOutput: SolcOutput, decodeSrc: SrcDecoder) {
  for (const node of solcOutput.sources[source].ast.nodes) {
    if (isNodeType('StructDefinition', node)) {
      const storageLocation = getStorageLocationAnnotation(node);
      if (storageLocation !== undefined) {
        throw new UpgradesError(
          `${decodeSrc(node)}: Namespace struct ${node.name} is defined outside of a contract`,
          () =>
            `Structs with the @custom:storage-location annotation must be defined within a contract. Move the struct definition into a contract, or remove the annotation if the struct is not used for namespaced storage.`,
        );
      }
    }
  }
}

function getNamespacedCompilationContext(
  source: string,
  contractDef: ContractDefinition,
  namespacedOutput?: SolcOutput,
) {
  if (namespacedOutput === undefined || contractDef.canonicalName === undefined) {
    return undefined;
  }

  if (namespacedOutput.sources[source] === undefined) {
    throw new Error(`Source ${source} not found in namespaced solc output`);
  }

  const namespacedContractDef = namespacedOutput.sources[source].ast.nodes
    .filter(isNodeType('ContractDefinition'))
    .find(c => c.canonicalName === contractDef.canonicalName);

  if (namespacedContractDef === undefined) {
    throw new Error(`Contract definition with name ${contractDef.canonicalName} not found in namespaced solc output`);
  }

  const storageLayout = namespacedOutput.contracts[source][contractDef.name].storageLayout;
  if (storageLayout === undefined) {
    throw new Error(`Storage layout for contract ${contractDef.canonicalName} not found in namespaced solc output`);
  }

  return {
    contractDef: namespacedContractDef,
    deref: astDereferencer(namespacedOutput),
    storageLayout: storageLayout,
  };
}

function* getConstructorErrors(contractDef: ContractDefinition, decodeSrc: SrcDecoder): Generator<ValidationError> {
  for (const fnDef of findAll('FunctionDefinition', contractDef, node => skipCheck('constructor', node))) {
    if (fnDef.kind === 'constructor' && ((fnDef.body?.statements?.length ?? 0) > 0 || fnDef.modifiers.length > 0)) {
      yield {
        kind: 'constructor',
        contract: contractDef.name,
        src: decodeSrc(fnDef),
      };
    }
  }
}

function initOpcodeCache(): OpcodeCache {
  return {
    mainContractErrors: new Map(),
    inheritedContractErrors: new Map(),
  };
}

interface OpcodeCache {
  mainContractErrors: Map<number, ValidationErrorOpcode[]>;
  inheritedContractErrors: Map<number, ValidationErrorOpcode[]>;
}

function* getOpcodeErrors(
  contractDef: ContractDefinition,
  deref: ASTDereferencer,
  decodeSrc: SrcDecoder,
  delegateCallCache: OpcodeCache,
  selfDestructCache: OpcodeCache,
): Generator<ValidationErrorOpcode, void, undefined> {
  yield* getContractOpcodeErrors(contractDef, deref, decodeSrc, OPCODES.delegatecall, 'main', delegateCallCache);
  yield* getContractOpcodeErrors(contractDef, deref, decodeSrc, OPCODES.selfdestruct, 'main', selfDestructCache);
}

/**
 * Whether this node is being visited as part of a main contract, or as an inherited contract or function
 */
type Scope = 'main' | 'inherited';

function* getContractOpcodeErrors(
  contractDef: ContractDefinition,
  deref: ASTDereferencer,
  decodeSrc: SrcDecoder,
  opcode: OpcodePattern,
  scope: Scope,
  cache: OpcodeCache,
): Generator<ValidationErrorOpcode, void, undefined> {
  const cached = getCachedOpcodes(contractDef.id, scope, cache);
  if (cached !== undefined) {
    yield* cached;
  } else {
    const errors: ValidationErrorOpcode[] = [];
    setCachedOpcodes(contractDef.id, scope, cache, errors);
    errors.push(
      ...getFunctionOpcodeErrors(contractDef, deref, decodeSrc, opcode, scope, cache),
      ...getInheritedContractOpcodeErrors(contractDef, deref, decodeSrc, opcode, cache),
    );
    yield* errors;
  }
}

function getCachedOpcodes(key: number, scope: string, cache: OpcodeCache) {
  return scope === 'main' ? cache.mainContractErrors.get(key) : cache.inheritedContractErrors.get(key);
}

function* getFunctionOpcodeErrors(
  contractOrFunctionDef: ContractDefinition | FunctionDefinition,
  deref: ASTDereferencer,
  decodeSrc: SrcDecoder,
  opcode: OpcodePattern,
  scope: Scope,
  cache: OpcodeCache,
): Generator<ValidationErrorOpcode, void, undefined> {
  const parentContractDef = getParentDefinition(deref, contractOrFunctionDef);
  if (parentContractDef === undefined || !skipCheck(opcode.kind, parentContractDef)) {
    yield* getDirectFunctionOpcodeErrors(contractOrFunctionDef, decodeSrc, opcode, scope);
  }
  if (parentContractDef === undefined || !skipCheckReachable(opcode.kind, parentContractDef)) {
    yield* getReferencedFunctionOpcodeErrors(contractOrFunctionDef, deref, decodeSrc, opcode, scope, cache);
  }
}

function* getDirectFunctionOpcodeErrors(
  contractOrFunctionDef: ContractDefinition | FunctionDefinition,
  decodeSrc: SrcDecoder,
  opcode: OpcodePattern,
  scope: Scope,
) {
  for (const fnCall of findAll(
    'FunctionCall',
    contractOrFunctionDef,
    node => skipCheck(opcode.kind, node) || (scope === 'inherited' && isInternalFunction(node)),
  )) {
    const fn = fnCall.expression;
    if (fn.typeDescriptions.typeIdentifier?.match(opcode.pattern)) {
      yield {
        kind: opcode.kind,
        src: decodeSrc(fnCall),
      };
    }
  }
}

function* getReferencedFunctionOpcodeErrors(
  contractOrFunctionDef: ContractDefinition | FunctionDefinition,
  deref: ASTDereferencer,
  decodeSrc: SrcDecoder,
  opcode: OpcodePattern,
  scope: Scope,
  cache: OpcodeCache,
) {
  for (const fnCall of findAll(
    'FunctionCall',
    contractOrFunctionDef,
    node => skipCheckReachable(opcode.kind, node) || (scope === 'inherited' && isInternalFunction(node)),
  )) {
    const fn = fnCall.expression;
    if ('referencedDeclaration' in fn && fn.referencedDeclaration && fn.referencedDeclaration > 0) {
      // non-positive references refer to built-in functions
      const referencedNode = tryDerefFunction(deref, fn.referencedDeclaration);
      if (referencedNode !== undefined) {
        const cached = getCachedOpcodes(referencedNode.id, scope, cache);
        if (cached !== undefined) {
          yield* cached;
        } else {
          const errors: ValidationErrorOpcode[] = [];
          setCachedOpcodes(referencedNode.id, scope, cache, errors);
          errors.push(...getFunctionOpcodeErrors(referencedNode, deref, decodeSrc, opcode, scope, cache));
          yield* errors;
        }
      }
    }
  }
}

function setCachedOpcodes(key: number, scope: string, cache: OpcodeCache, errors: ValidationErrorOpcode[]) {
  if (scope === 'main') {
    cache.mainContractErrors.set(key, errors);
  } else {
    cache.inheritedContractErrors.set(key, errors);
  }
}

function tryDerefFunction(deref: ASTDereferencer, referencedDeclaration: number): FunctionDefinition | undefined {
  try {
    return deref(['FunctionDefinition'], referencedDeclaration);
  } catch (e: any) {
    if (!e.message.includes('No node with id')) {
      throw e;
    }
  }
}

function tryDerefStruct(deref: ASTDereferencer, referencedDeclaration: number): StructDefinition | undefined {
  try {
    return deref(['StructDefinition'], referencedDeclaration);
  } catch (e: any) {
    if (!e.message.includes('No node with id')) {
      throw e;
    }
  }
}

function* getInheritedContractOpcodeErrors(
  contractDef: ContractDefinition,
  deref: ASTDereferencer,
  decodeSrc: SrcDecoder,
  opcode: OpcodePattern,
  cache: OpcodeCache,
) {
  if (!skipCheckReachable(opcode.kind, contractDef)) {
    for (const base of contractDef.baseContracts) {
      const referencedContract = deref('ContractDefinition', base.baseName.referencedDeclaration);
      yield* getContractOpcodeErrors(referencedContract, deref, decodeSrc, opcode, 'inherited', cache);
    }
  }
}

function getParentDefinition(deref: ASTDereferencer, contractOrFunctionDef: ContractDefinition | FunctionDefinition) {
  const parentNode = deref(['ContractDefinition', 'SourceUnit'], contractOrFunctionDef.scope);
  if (parentNode.nodeType === 'ContractDefinition') {
    return parentNode;
  }
}

function isInternalFunction(node: Node) {
  return (
    node.nodeType === 'FunctionDefinition' &&
    node.kind !== 'constructor' && // do not consider constructors as internal, because they are always called by children contracts' constructors
    (node.visibility === 'internal' || node.visibility === 'private')
  );
}

function* getStateVariableErrors(
  contractDef: ContractDefinition,
  decodeSrc: SrcDecoder,
): Generator<ValidationErrorWithName> {
  for (const varDecl of contractDef.nodes) {
    if (isNodeType('VariableDeclaration', varDecl)) {
      if (varDecl.mutability === 'immutable') {
        if (!skipCheck('state-variable-immutable', contractDef) && !skipCheck('state-variable-immutable', varDecl)) {
          yield {
            kind: 'state-variable-immutable',
            name: varDecl.name,
            src: decodeSrc(varDecl),
          };
        }
      } else if (!varDecl.constant && !isNullish(varDecl.value)) {
        // Assignments are only a concern for non-immutable variables
        if (!skipCheck('state-variable-assignment', contractDef) && !skipCheck('state-variable-assignment', varDecl)) {
          yield {
            kind: 'state-variable-assignment',
            name: varDecl.name,
            src: decodeSrc(varDecl),
          };
        }
      }
    }
  }
}

function getReferencedLibraryIds(contractDef: ContractDefinition): number[] {
  const implicitUsage = [...findAll('UsingForDirective', contractDef)]
    .map(usingForDirective => {
      if (usingForDirective.libraryName !== undefined) {
        return usingForDirective.libraryName.referencedDeclaration;
      } else if (usingForDirective.functionList !== undefined) {
        return [];
      } else {
        throw new Error(
          'Broken invariant: either UsingForDirective.libraryName or UsingForDirective.functionList should be defined',
        );
      }
    })
    .flat();

  const explicitUsage = [...findAll('Identifier', contractDef)]
    .filter(identifier => identifier.typeDescriptions.typeString?.match(/^type\(library/))
    .map(identifier => {
      if (isNullish(identifier.referencedDeclaration)) {
        throw new Error('Broken invariant: Identifier.referencedDeclaration should not be null');
      }
      return identifier.referencedDeclaration;
    });

  return [...new Set(implicitUsage.concat(explicitUsage))];
}

function* getLinkingErrors(
  contractDef: ContractDefinition,
  bytecode: SolcBytecode,
): Generator<ValidationErrorWithName> {
  const { linkReferences } = bytecode;
  for (const source of Object.keys(linkReferences)) {
    for (const libName of Object.keys(linkReferences[source])) {
      if (!skipCheck('external-library-linking', contractDef)) {
        yield {
          kind: 'external-library-linking',
          name: libName,
          src: source,
        };
      }
    }
  }
}

function* getInternalFunctionStorageErrors(
  contractOrStructDef: ContractDefinition | StructDefinition,
  deref: ASTDereferencer,
  decodeSrc: SrcDecoder,
  visitedNodeIds = new Set<number>(),
): Generator<ValidationError> {
  // Note: Solidity does not allow annotations for non-public state variables, nor recursive types for public variables,
  // so annotations cannot be used to skip these checks.
  for (const variableDec of getVariableDeclarations(contractOrStructDef, visitedNodeIds)) {
    if (variableDec.typeName?.nodeType === 'FunctionTypeName' && variableDec.typeName.visibility === 'internal') {
      // Find internal function types directly in this node's scope
      yield {
        kind: 'internal-function-storage',
        name: variableDec.name,
        src: decodeSrc(variableDec),
      };
    } else if (variableDec.typeName) {
      const userDefinedType = findUserDefinedType(variableDec.typeName);
      // Recursively try to dereference struct since it may be declared elsewhere
      if (userDefinedType !== undefined && !visitedNodeIds.has(userDefinedType.referencedDeclaration)) {
        const structDef = tryDerefStruct(deref, userDefinedType.referencedDeclaration);
        if (structDef !== undefined) {
          visitedNodeIds.add(userDefinedType.referencedDeclaration);
          yield* getInternalFunctionStorageErrors(structDef, deref, decodeSrc, visitedNodeIds);
        }
      }
    }
  }
}

/**
 * Gets variables declared directly in a contract or in a struct definition.
 *
 * If this is a contract with struct definitions annotated with a storage location according to ERC-7201,
 * then the struct members are also included.
 */
function* getVariableDeclarations(
  contractOrStructDef: ContractDefinition | StructDefinition,
  visitedNodeIds: Set<number>,
): Generator<VariableDeclaration, void, undefined> {
  if (contractOrStructDef.nodeType === 'ContractDefinition') {
    for (const node of contractOrStructDef.nodes) {
      if (node.nodeType === 'VariableDeclaration') {
        yield node;
      } else if (
        node.nodeType === 'StructDefinition' &&
        getStorageLocationAnnotation(node) !== undefined &&
        !visitedNodeIds.has(node.id)
      ) {
        visitedNodeIds.add(node.id);
        yield* getVariableDeclarations(node, visitedNodeIds);
      }
    }
  } else if (contractOrStructDef.nodeType === 'StructDefinition') {
    yield* contractOrStructDef.members;
  }
}

/**
 * Recursively traverse array and mapping types to find user-defined types (which may be struct references).
 */
function findUserDefinedType(typeName: TypeName): UserDefinedTypeName | undefined {
  switch (typeName.nodeType) {
    case 'ArrayTypeName':
      return findUserDefinedType(typeName.baseType);
    case 'Mapping':
      // only mapping values can possibly refer to other array, mapping, or user-defined types
      return findUserDefinedType(typeName.valueType);
    case 'UserDefinedTypeName':
      return typeName;
    case 'ElementaryTypeName':
    case 'FunctionTypeName':
      return undefined;
    default:
      return assertUnreachable(typeName);
  }
}
