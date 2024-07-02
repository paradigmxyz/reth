export function isNullish(value: unknown): value is null | undefined {
  return value === null || value === undefined;
}
