import { Encoding, IEncoding } from './encoding'

export class BufferFormat<TIn, TOut> extends Encoding<TIn, Buffer, TOut> {
  constructor (options: Omit<IEncoding<TIn, Buffer, TOut>, 'format'>)
}

export class ViewFormat<TIn, TOut> extends Encoding<TIn, Uint8Array, TOut> {
  constructor (options: Omit<IEncoding<TIn, Uint8Array, TOut>, 'format'>)
}

export class UTF8Format<TIn, TOut> extends Encoding<TIn, string, TOut> {
  constructor (options: Omit<IEncoding<TIn, string, TOut>, 'format'>)
}
