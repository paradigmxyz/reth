import { Encoding, MixedEncoding, KnownEncoding, KnownEncodingName } from './lib/encoding'

export class Transcoder<T = any> {
  /**
   * Create a Transcoder.
   * @param formats Formats supported by consumer.
   */
  constructor (formats: Array<'buffer'|'view'|'utf8'>)

  /**
   * Get an array of supported encoding objects.
   */
  encodings (): Array<Encoding<any, T, any>>

  /**
   * Get the given encoding, creating a transcoder encoding if necessary.
   * @param encoding Named encoding or encoding object.
   */
  encoding<TIn, TFormat, TOut> (
    encoding: MixedEncoding<TIn, TFormat, TOut>
  ): Encoding<TIn, T, TOut>

  encoding<N extends KnownEncodingName> (encoding: N): KnownEncoding<N, T>
  encoding (encoding: string): Encoding<any, T, any>
}

export * from './lib/encoding'
