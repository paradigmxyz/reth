export function makeNonEnumerable<O>(obj: O, key: keyof O): void {
  Object.defineProperty(obj, key, { enumerable: false });
}
