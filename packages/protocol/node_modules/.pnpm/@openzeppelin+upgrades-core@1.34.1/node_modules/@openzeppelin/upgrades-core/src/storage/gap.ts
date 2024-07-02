import { StorageField, storageFieldEnd } from './compare';

/**
 * Returns true if the field represents a storage gap.
 *
 * @param field the storage field
 * @returns true if field is a gap, otherwise false
 */
export function isGap(field: StorageField): boolean {
  return field.type.head === 't_array' && (field.label === '__gap' || field.label.startsWith('__gap_'));
}

/**
 * Returns true if original storage field is a gap and the updated storage field
 * ends at the exact same position as the gap.
 *
 * @param original the original storage field
 * @param updated the updated storage field
 * @returns true if original is a gap and original and updated end at the same position, otherwise false
 */
export function endMatchesGap(original: StorageField, updated: StorageField) {
  const originalEnd = storageFieldEnd(original);
  const updatedEnd = storageFieldEnd(updated);

  return isGap(original) && originalEnd !== undefined && updatedEnd !== undefined && originalEnd === updatedEnd;
}
