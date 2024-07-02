import { ParsedTypeDetailed } from '../storage/layout';

export function isValueType(type: ParsedTypeDetailed): boolean {
  return type.args === undefined || ['t_contract', 't_enum', 't_userDefinedValueType'].includes(type.head);
}
