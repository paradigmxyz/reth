export function fromCallback<T, S extends symbol>(callback: Callback<T> | undefined, symbol: S)
  : Callback<T> & { [K in S]?: Promise<T> };

export function fromCallback<T>(callback: Callback<T> | undefined)
  : Callback<T> & { promise?: Promise<T> };

export function fromPromise<T>(promise: Promise<T>, callback: Callback<T> | undefined)
  : Promise<T> | undefined;

type Callback<T> = (err: Error | null, result: T) => void;
