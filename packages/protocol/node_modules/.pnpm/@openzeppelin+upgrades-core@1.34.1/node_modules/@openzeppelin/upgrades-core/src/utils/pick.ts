export function pick<T, K extends keyof T>(obj: T, keys: K[]): Pick<T, K> {
  const res: Partial<Pick<T, K>> = {};
  for (const k of keys) {
    res[k] = obj[k];
  }
  return res as Pick<T, K>;
}
