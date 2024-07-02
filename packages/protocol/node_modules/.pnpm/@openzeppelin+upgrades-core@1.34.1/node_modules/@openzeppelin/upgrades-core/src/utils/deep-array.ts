export type DeepArray<T> = T | DeepArray<T>[];

export function deepEqual<T>(a: DeepArray<T>, b: DeepArray<T>): boolean {
  if (Array.isArray(a) && Array.isArray(b)) {
    return a.length === b.length && a.every((v, i) => deepEqual(v, b[i]));
  } else {
    return a === b;
  }
}
