import { Cost, levenshtein, Operation } from '../levenshtein';
import { hasLayout, ParsedTypeDetailed, isEnumMembers, isStructMembers } from './layout';
import { UpgradesError } from '../error';
import { StorageItem as _StorageItem, StructMember as _StructMember, StorageField as _StorageField } from './layout';
import { LayoutCompatibilityReport } from './report';
import { assert } from '../utils/assert';
import { isValueType } from '../utils/is-value-type';
import { endMatchesGap, isGap } from './gap';

export type StorageItem = _StorageItem<ParsedTypeDetailed>;
type StructMember = _StructMember<ParsedTypeDetailed>;

export type StorageField = _StorageField<ParsedTypeDetailed>;
export type StorageOperation<F extends StorageField> = Operation<F, StorageFieldChange<F>>;

export type EnumOperation = Operation<string, { kind: 'replace'; original: string; updated: string }>;

type StorageFieldChange<F extends StorageField> = (
  | { kind: 'replace' | 'rename' | 'finishgap' }
  | { kind: 'typechange'; change: TypeChange }
  | { kind: 'layoutchange'; change: LayoutChange }
  | { kind: 'shrinkgap'; change: TypeChange }
) & {
  original: F;
  updated: F;
} & Cost;

export type TypeChange = (
  | {
      kind:
        | 'obvious mismatch'
        | 'unknown'
        | 'array grow'
        | 'visibility change'
        | 'array shrink'
        | 'array dynamic'
        | 'type resize'
        | 'missing members';
    }
  | {
      kind: 'mapping key' | 'mapping value' | 'array value';
      inner: TypeChange;
    }
  | {
      kind: 'enum members';
      ops: EnumOperation[];
    }
  | {
      kind: 'struct members';
      ops: StorageOperation<StructMember>[];
      allowAppend: boolean;
    }
) & {
  original: ParsedTypeDetailed;
  updated: ParsedTypeDetailed;
};

export interface LayoutChange {
  uncertain?: boolean;
  knownCompatible?: boolean;
  slot?: Record<'from' | 'to', string>;
  offset?: Record<'from' | 'to', number>;
  bytes?: Record<'from' | 'to', string>;
}

/**
 * Gets the storage field's begin position as the number of bytes from 0.
 *
 * @param field the storage field
 * @returns the begin position, or undefined if the slot or offset is undefined
 */
export function storageFieldBegin(field: StorageField): bigint | undefined {
  if (field.slot === undefined || field.offset === undefined) {
    return undefined;
  }
  return BigInt(field.slot) * 32n + BigInt(field.offset);
}

/**
 * Gets the storage field's end position as the number of bytes from 0.
 *
 * @param field the storage field
 * @returns the end position, or undefined if the slot or offset or number of bytes is undefined
 */
export function storageFieldEnd(field: StorageField): bigint | undefined {
  const begin = storageFieldBegin(field);
  const numberOfBytes = field.type.item.numberOfBytes;
  if (begin === undefined || numberOfBytes === undefined) {
    return undefined;
  }
  return begin + BigInt(numberOfBytes);
}

const LAYOUTCHANGE_COST = 1;
const FINISHGAP_COST = 1;
const SHRINKGAP_COST = 0;
const TYPECHANGE_COST = 1;

export class StorageLayoutComparator {
  hasAllowedUncheckedCustomTypes = false;

  // Holds a stack of type comparisons to detect recursion
  stack = new Set<string>();
  cache = new Map<string, TypeChange | undefined>();

  constructor(
    readonly unsafeAllowCustomTypes = false,
    readonly unsafeAllowRenames = false,
  ) {}

  compareLayouts(
    original: StorageItem[],
    updated: StorageItem[],
    originalNamespaces?: Record<string, StorageItem[]>,
    updatedNamespaces?: Record<string, StorageItem[]>,
  ): LayoutCompatibilityReport {
    const ops = this.layoutLevenshtein(original, updated, { allowAppend: true });
    const namespacedOps = this.getNamespacedStorageOperations(originalNamespaces, updatedNamespaces);

    return new LayoutCompatibilityReport([...ops, ...namespacedOps]);
  }

  private getNamespacedStorageOperations(
    originalNamespaces?: Record<string, StorageItem[]>,
    updatedNamespaces?: Record<string, StorageItem[]>,
  ) {
    const ops: StorageOperation<StorageItem>[] = [];
    if (originalNamespaces !== undefined) {
      for (const [storageLocation, origNamespacedLayout] of Object.entries(originalNamespaces)) {
        const updatedNamespacedLayout = updatedNamespaces?.[storageLocation];
        if (updatedNamespacedLayout !== undefined) {
          ops.push(...this.layoutLevenshtein(origNamespacedLayout, updatedNamespacedLayout, { allowAppend: true }));
        } else if (origNamespacedLayout.length > 0) {
          ops.push({
            kind: 'delete-namespace',
            namespace: storageLocation,
            original: {
              contract: origNamespacedLayout[0].contract,
            },
          });
        }
      }
    }
    return ops;
  }

  private layoutLevenshtein<F extends StorageField>(
    original: F[],
    updated: F[],
    { allowAppend }: { allowAppend: boolean },
  ): StorageOperation<F>[] {
    const ops = levenshtein(original, updated, (a, b) => this.getFieldChange(a, b));

    // filter operations: the callback function returns true if operation is unsafe, false if safe
    return ops.filter(o => {
      if (o.kind === 'insert') {
        const updatedStart = storageFieldBegin(o.updated);
        const updatedEnd = storageFieldEnd(o.updated);
        // If we cannot get the updated position, or if any entry in the original layout (that is not a gap) overlaps, then the insertion is unsafe.
        return (
          updatedStart === undefined ||
          updatedEnd === undefined ||
          original
            .filter(entry => !isGap(entry))
            .some(entry => {
              const origStart = storageFieldBegin(entry);
              const origEnd = storageFieldEnd(entry);
              return (
                origStart === undefined ||
                origEnd === undefined ||
                overlaps(origStart, origEnd, updatedStart, updatedEnd)
              );
            })
        );
      } else if ((o.kind === 'shrinkgap' || o.kind === 'finishgap') && endMatchesGap(o.original, o.updated)) {
        // Gap shrink or gap replacement that finishes on the same slot is safe
        return false;
      } else if (o.kind === 'append' && allowAppend) {
        return false;
      }
      return true;
    });

    // https://stackoverflow.com/questions/325933/determine-whether-two-date-ranges-overlap
    function overlaps(startA: bigint, endA: bigint, startB: bigint, endB: bigint) {
      return startA < endB && startB < endA;
    }
  }

  getVisibilityChange(original: ParsedTypeDetailed, updated: ParsedTypeDetailed): TypeChange | undefined {
    const re = /^t_function_(internal|external)/;
    const originalVisibility = original.head.match(re);
    const updatedVisibility = updated.head.match(re);
    assert(originalVisibility && updatedVisibility);
    if (originalVisibility[0] !== updatedVisibility[0]) {
      return { kind: 'visibility change', original, updated };
    }
  }

  getFieldChange<F extends StorageField>(original: F, updated: F): StorageFieldChange<F> | undefined {
    const nameChange =
      !this.unsafeAllowRenames &&
      original.label !== updated.renamedFrom &&
      (updated.label !== original.label ||
        (updated.renamedFrom !== undefined && updated.renamedFrom !== original.renamedFrom));
    const typeChange =
      !this.isRetypedFromOriginal(original, updated) &&
      this.getTypeChange(original.type, updated.type, { allowAppend: false });
    const layoutChange = this.getLayoutChange(original, updated);

    if (updated.retypedFrom && layoutChange && (!layoutChange.uncertain || !layoutChange.knownCompatible)) {
      return { kind: 'layoutchange', original, updated, change: layoutChange, cost: LAYOUTCHANGE_COST };
    } else if (nameChange && endMatchesGap(original, updated)) {
      // original is a gap that was consumed by a non-gap ending at the same slot, which is safe
      return { kind: 'finishgap', original, updated, cost: FINISHGAP_COST };
    } else if (typeChange && typeChange.kind === 'array shrink' && endMatchesGap(original, updated)) {
      // original and updated are a gap that was shrunk and ends at the same slot, which is safe
      return { kind: 'shrinkgap', change: typeChange, original, updated, cost: SHRINKGAP_COST };
    } else if (typeChange && nameChange) {
      return { kind: 'replace', original, updated };
    } else if (nameChange) {
      return { kind: 'rename', original, updated };
    } else if (typeChange) {
      return { kind: 'typechange', change: typeChange, original, updated, cost: TYPECHANGE_COST };
    } else if (layoutChange && !layoutChange.uncertain) {
      // Any layout change should be caught earlier as a type change, but we
      // add this check as a safety fallback.
      return { kind: 'layoutchange', original, updated, change: layoutChange, cost: LAYOUTCHANGE_COST };
    }
  }

  private isRetypedFromOriginal(original: StorageField, updated: StorageField) {
    const originalLabel = stripContractSubstrings(original.type.item.label);
    const updatedLabel = stripContractSubstrings(updated.retypedFrom?.trim());

    return originalLabel === updatedLabel;
  }

  getLayoutChange(original: StorageField, updated: StorageField): LayoutChange | undefined {
    if (hasLayout(original) && hasLayout(updated)) {
      const change = <T>(from: T, to: T) => (from === to ? undefined : { from, to });
      const slot = change(original.slot, updated.slot);
      const offset = change(original.offset, updated.offset);
      const bytes = change(original.type.item.numberOfBytes, updated.type.item.numberOfBytes);
      if (slot || offset || bytes) {
        return { slot, offset, bytes };
      }
    } else {
      const validChanges = [['uint8', 'bool']];
      const knownCompatible = validChanges.some(
        group => group.includes(original.type.item.label) && group.includes(updated.type.item.label),
      );
      return { uncertain: true, knownCompatible };
    }
  }

  getTypeChange(
    original: ParsedTypeDetailed,
    updated: ParsedTypeDetailed,
    { allowAppend }: { allowAppend: boolean },
  ): TypeChange | undefined {
    const key = JSON.stringify({ original: original.id, updated: updated.id, allowAppend });
    if (this.cache.has(key)) {
      return this.cache.get(key);
    }

    if (this.stack.has(key)) {
      throw new UpgradesError(`Recursive types are not supported`, () => `Recursion found in ${updated.item.label}\n`);
    }

    try {
      this.stack.add(key);
      const result = this.uncachedGetTypeChange(original, updated, { allowAppend });
      this.cache.set(key, result);
      return result;
    } finally {
      this.stack.delete(key);
    }
  }

  private uncachedGetTypeChange(
    original: ParsedTypeDetailed,
    updated: ParsedTypeDetailed,
    { allowAppend }: { allowAppend: boolean },
  ): TypeChange | undefined {
    if (original.head.startsWith('t_function') && updated.head.startsWith('t_function')) {
      return this.getVisibilityChange(original, updated);
    }

    if (
      (original.head === 't_contract' && updated.head === 't_address') ||
      (original.head === 't_address' && updated.head === 't_contract')
    ) {
      // changing contract to address or address to contract
      // equivalent to just addresses
      return undefined;
    }

    if (normalizeMemoryPointer(original.head) !== normalizeMemoryPointer(updated.head)) {
      return { kind: 'obvious mismatch', original, updated };
    }

    if (original.args === undefined || updated.args === undefined) {
      // both should be undefined at the same time
      assert(original.args === updated.args);
      return undefined;
    }

    switch (original.head) {
      case 't_contract':
        // changing contract to contract
        // no storage layout errors can be introduced here since it is just an address
        return undefined;

      case 't_struct': {
        const originalMembers = original.item.members;
        const updatedMembers = updated.item.members;
        if (originalMembers === undefined || updatedMembers === undefined) {
          if (this.unsafeAllowCustomTypes) {
            this.hasAllowedUncheckedCustomTypes = true;
            return undefined;
          } else {
            return { kind: 'missing members', original, updated };
          }
        }
        assert(isStructMembers(originalMembers) && isStructMembers(updatedMembers));
        const ops = this.layoutLevenshtein(originalMembers, updatedMembers, { allowAppend });
        if (ops.length > 0) {
          return { kind: 'struct members', ops, original, updated, allowAppend };
        } else {
          return undefined;
        }
      }

      case 't_enum': {
        const originalMembers = original.item.members;
        const updatedMembers = updated.item.members;
        if (originalMembers === undefined || updatedMembers === undefined) {
          if (this.unsafeAllowCustomTypes) {
            this.hasAllowedUncheckedCustomTypes = true;
            return undefined;
          } else {
            return { kind: 'missing members', original, updated };
          }
        }
        assert(isEnumMembers(originalMembers) && isEnumMembers(updatedMembers));
        if (enumSize(originalMembers.length) !== enumSize(updatedMembers.length)) {
          return { kind: 'type resize', original, updated };
        } else {
          const ops = levenshtein(originalMembers, updatedMembers, (a, b) =>
            a === b ? undefined : { kind: 'replace' as const, original: a, updated: b },
          ).filter(o => o.kind !== 'append');
          if (ops.length > 0) {
            return { kind: 'enum members', ops, original, updated };
          } else {
            return undefined;
          }
        }
      }

      case 't_mapping': {
        const [originalKey, originalValue] = original.args;
        const [updatedKey, updatedValue] = updated.args;

        // validate an invariant we assume from solidity: key types are always simple value types
        assert(isValueType(originalKey) && isValueType(updatedKey));

        // network files migrated from the OZ CLI have an unknown key type
        // we allow it to match with any other key type, carrying over the semantics of OZ CLI
        const keyChange =
          originalKey.head === 'unknown'
            ? undefined
            : this.getTypeChange(originalKey, updatedKey, { allowAppend: false });

        if (keyChange) {
          return { kind: 'mapping key', inner: keyChange, original, updated };
        } else {
          // mapping value types are allowed to grow
          const inner = this.getTypeChange(originalValue, updatedValue, { allowAppend: true });
          if (inner) {
            return { kind: 'mapping value', inner, original, updated };
          } else {
            return undefined;
          }
        }
      }

      case 't_array': {
        const originalLength = original.tail?.match(/^(\d+|dyn)/)?.[0];
        const updatedLength = updated.tail?.match(/^(\d+|dyn)/)?.[0];
        assert(originalLength !== undefined && updatedLength !== undefined);

        if (originalLength === 'dyn' || updatedLength === 'dyn') {
          if (originalLength !== updatedLength) {
            return { kind: 'array dynamic', original, updated };
          }
        }

        const originalLengthInt = parseInt(originalLength, 10);
        const updatedLengthInt = parseInt(updatedLength, 10);

        if (updatedLengthInt < originalLengthInt) {
          return { kind: 'array shrink', original, updated };
        } else if (!allowAppend && updatedLengthInt > originalLengthInt) {
          return { kind: 'array grow', original, updated };
        }

        const inner = this.getTypeChange(original.args[0], updated.args[0], { allowAppend: false });

        if (inner) {
          return { kind: 'array value', inner, original, updated };
        } else {
          return undefined;
        }
      }

      case 't_userDefinedValueType': {
        const underlyingMatch =
          original.item.underlying !== undefined &&
          updated.item.underlying !== undefined &&
          original.item.underlying.id === updated.item.underlying.id;

        if (
          (original.item.numberOfBytes === undefined || updated.item.numberOfBytes === undefined) &&
          !underlyingMatch
        ) {
          return { kind: 'unknown', original, updated };
        }
        if (original.item.numberOfBytes !== updated.item.numberOfBytes) {
          return { kind: 'type resize', original, updated };
        } else {
          return undefined;
        }
      }

      default:
        return { kind: 'unknown', original, updated };
    }
  }
}

function enumSize(memberCount: number): number {
  return Math.ceil(Math.log2(Math.max(2, memberCount)) / 8);
}

export function stripContractSubstrings(label?: string) {
  if (label !== undefined) {
    return label.replace(/\b(contract|struct|enum) /g, '');
  }
}

/**
 * Some versions of Solidity use type ids with _memory_ptr suffix while other versions use _memory suffix.
 * Normalize these for type comparison purposes only.
 */
function normalizeMemoryPointer(typeIdentifier: string): string {
  return typeIdentifier.replace(/_memory_ptr\b/g, '_memory');
}
