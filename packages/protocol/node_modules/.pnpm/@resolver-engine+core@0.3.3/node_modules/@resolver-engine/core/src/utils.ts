import { Context } from ".";

export async function firstResult<T, R>(
  things: T[],
  check: (thing: T) => Promise<R | null>,
  ctx?: Context,
): Promise<{ result: R; index: number } | null> {
  for (let index = 0; index < things.length; index++) {
    const result = await check(things[index]);
    if (result) {
      return { result, index };
    }
  }
  return null;
}
