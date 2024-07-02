import type { FunctionDefinition, TypeName, VariableDeclaration } from 'solidity-ast';
import { ASTDereferencer } from 'solidity-ast/utils';
import { assert, assertUnreachable } from './assert';

function serializeParameterType(parameter: VariableDeclaration, deref: ASTDereferencer): string {
  const { typeName, storageLocation } = parameter;
  assert(!!typeName);

  if (storageLocation === 'storage') {
    assert(typeof typeName.typeDescriptions.typeString === 'string');
    return typeName.typeDescriptions.typeString.replace(/^struct /, '') + ' storage';
  }

  return serializeTypeName(typeName, deref);
}

function serializeTypeName(typeName: TypeName, deref: ASTDereferencer): string {
  switch (typeName.nodeType) {
    case 'ArrayTypeName':
    case 'ElementaryTypeName': {
      assert(typeof typeName.typeDescriptions.typeString === 'string');
      return typeName.typeDescriptions.typeString;
    }

    case 'UserDefinedTypeName': {
      const userDefinedType = deref(
        ['StructDefinition', 'EnumDefinition', 'ContractDefinition', 'UserDefinedValueTypeDefinition'],
        typeName.referencedDeclaration,
      );
      switch (userDefinedType.nodeType) {
        case 'StructDefinition':
          return '(' + userDefinedType.members.map(member => serializeParameterType(member, deref)) + ')';

        case 'EnumDefinition':
          assert(userDefinedType.members.length < 256);
          return 'uint8';

        case 'ContractDefinition':
          return 'address';

        case 'UserDefinedValueTypeDefinition':
          return serializeTypeName(userDefinedType.underlyingType, deref);

        default:
          return assertUnreachable(userDefinedType);
      }
    }

    case 'FunctionTypeName': {
      return `function`;
    }

    default:
      throw new Error(`Unsuported TypeName node type: ${typeName.nodeType}`);
  }
}

export function getFunctionSignature(fnDef: FunctionDefinition, deref: ASTDereferencer): string {
  return `${fnDef.name}(${fnDef.parameters.parameters
    .map(parameter => serializeParameterType(parameter, deref))
    .join()})`;
}
