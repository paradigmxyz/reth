declare module 'array.prototype.findlast' {
  function findLast<T, S extends T>(arr: readonly T[], predicate: (value: T, index: number, array: T[]) => value is S, thisArg?: any): S | undefined;
  function findLast<T>(arr: readonly T[], predicate: (value: T, index: number, array: T[]) => unknown, thisArg?: any): T | undefined;

  export default findLast;
}
