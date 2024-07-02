/**
 * An error-first callback in the style of Node.js.
 */
export type NodeCallback<T> =
  (err: Error | undefined | null, result?: T | undefined) => void

export interface RangeOptions<K> {
  gt?: K
  gte?: K
  lt?: K
  lte?: K
  reverse?: boolean | undefined
  limit?: number | undefined
}
