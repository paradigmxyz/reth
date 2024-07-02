import { parseTypeId, ParsedTypeId } from '../utils/parse-type-id';

// The interfaces below are generic in the way types are represented (through the parameter `Type`). When stored on
// disk, the type is represented by a string: the type id. When loaded onto memory to run the storage layout comparisons
// found in this module, the type id is replaced by its parsed structure together with the corresponding TypeItem, e.g.
// the struct members if it is a struct type.

export interface StorageLayout {
  layoutVersion?: string;
  solcVersion?: string;
  storage: StorageItem[];
  types: Record<string, TypeItem>;
  flat?: boolean;
  namespaces?: Record<string, StorageItem[]>;
}

export type StorageField<Type = string> = StorageItem<Type> | StructMember<Type>;

export interface StorageItem<Type = string> {
  astId?: number;
  contract: string;
  label: string;
  type: Type;
  src: string;
  offset?: number;
  slot?: string;
  retypedFrom?: string;
  renamedFrom?: string;
}

export interface TypeItem<Type = string> {
  label: string;
  members?: TypeItemMembers<Type>;
  numberOfBytes?: string;
  underlying?: Type;
}

export type TypeItemMembers<Type = string> = StructMember<Type>[] | EnumMember[];

export interface StructMember<Type = string> {
  label: string;
  type: Type;
  retypedFrom?: string;
  renamedFrom?: string;
  offset?: number;
  slot?: string;
}

export type EnumMember = string;

export interface ParsedTypeDetailed extends ParsedTypeId {
  item: TypeItem<ParsedTypeDetailed>;
  args?: ParsedTypeDetailed[];
  rets?: ParsedTypeDetailed[];
}

type Replace<T, K extends string, V> = Omit<T, K> & Record<K, V>;

export function getDetailedLayout(layout: StorageLayout): StorageItem<ParsedTypeDetailed>[] {
  const cache: Record<string, ParsedTypeDetailed> = {};
  return layout.storage.map(parseItemWithDetails);

  function parseItemWithDetails<I extends { type: string }>(item: I): Replace<I, 'type', ParsedTypeDetailed> {
    return { ...item, type: parseIdWithDetails(item.type) };
  }

  function parseIdWithDetails(typeId: string): ParsedTypeDetailed {
    const parsed = parseTypeId(typeId);
    return addDetailsToParsedType(parsed);
  }

  function addDetailsToParsedType(parsed: ParsedTypeId): ParsedTypeDetailed {
    if (parsed.id in cache) {
      return cache[parsed.id];
    }

    const item = layout.types[parsed.id];
    const detailed: ParsedTypeDetailed = {
      ...parsed,
      args: undefined,
      rets: undefined,
      item: {
        ...item,
        members: undefined,
        underlying: undefined,
      },
    };

    // store in cache before recursion below
    cache[parsed.id] = detailed;

    detailed.args = parsed.args?.map(addDetailsToParsedType);
    detailed.rets = parsed.rets?.map(addDetailsToParsedType);
    detailed.item.members =
      item?.members && (isStructMembers(item?.members) ? item.members.map(parseItemWithDetails) : item?.members);
    detailed.item.underlying = item?.underlying === undefined ? undefined : parseIdWithDetails(item.underlying);

    return detailed;
  }
}

export function isEnumMembers<T>(members: TypeItemMembers<T>): members is EnumMember[] {
  return members.length === 0 || typeof members[0] === 'string';
}

export function isStructMembers<T>(members: TypeItemMembers<T>): members is StructMember<T>[] {
  return members.length === 0 || typeof members[0] === 'object';
}

export type StorageFieldWithLayout = StorageField<ParsedTypeDetailed> &
  Required<Pick<StorageField, 'offset' | 'slot'>> & { type: { item: Required<Pick<TypeItem, 'numberOfBytes'>> } };

export function hasLayout(field: StorageField<ParsedTypeDetailed>): field is StorageFieldWithLayout {
  return field.offset !== undefined && field.slot !== undefined && field.type.item.numberOfBytes !== undefined;
}
