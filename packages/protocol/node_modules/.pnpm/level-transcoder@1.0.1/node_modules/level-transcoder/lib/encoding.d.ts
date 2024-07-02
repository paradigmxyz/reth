import { BufferFormat, ViewFormat, UTF8Format } from './formats'

/**
 * Encodes {@link TIn} to {@link TFormat} and decodes
 * {@link TFormat} to {@link TOut}.
 */
export abstract class Encoding<TIn, TFormat, TOut> implements IEncoding<TIn, TFormat, TOut> {
  constructor (options: IEncoding<TIn, TFormat, TOut>)

  encode: (data: TIn) => TFormat
  decode: (data: TFormat) => TOut
  name: string
  format: 'buffer' | 'view' | 'utf8'
  createViewTranscoder (): ViewFormat<TIn, TOut>
  createBufferTranscoder (): BufferFormat<TIn, TOut>
  createUTF8Transcoder (): UTF8Format<TIn, TOut>

  /**
   * Common name, computed from {@link name}. If this encoding is a
   * transcoder encoding, {@link name} will be for example 'json+view'
   * and {@link commonName} will be just 'json'. Else {@link name}
   * will equal {@link commonName}.
   */
  get commonName (): string
}

export interface IEncoding<TIn, TFormat, TOut> {
  /**
   * Encode data.
   */
  encode: (data: TIn) => TFormat

  /**
   * Decode data.
   */
  decode: (data: TFormat) => TOut

  /**
   * Unique name.
   */
  name: string

  /**
   * The name of the (lower-level) encoding used by the return value of
   * {@link encode}. One of 'buffer', 'view', 'utf8'.
   */
  format: 'buffer' | 'view' | 'utf8'

  /**
   * Create a new encoding that transcodes {@link TFormat} from / to a view.
   */
  createViewTranscoder?: (() => ViewFormat<TIn, TOut>) | undefined

  /**
   * Create a new encoding that transcodes {@link TFormat} from / to a buffer.
   */
  createBufferTranscoder?: (() => BufferFormat<TIn, TOut>) | undefined

  /**
   * Create a new encoding that transcodes {@link TFormat} from / to a UTF-8 string.
   */
  createUTF8Transcoder?: (() => UTF8Format<TIn, TOut>) | undefined
}

export interface IExternalEncoding<TIn, TFormat, TOut> {
  /**
   * Encode data.
   */
  encode: (data: TIn) => TFormat

  /**
   * Decode data.
   */
  decode: (data: TFormat) => TOut

  /**
   * Unique name.
   */
  name?: string | undefined

  /**
   * Legacy `level-codec` option that means the same as `format: 'buffer'`
   * if true or `format: 'utf8'` if false.
   */
  buffer?: boolean | undefined

  /**
   * Legacy `level-codec` alias for {@link name}. Used only when the
   * {@link name} option is undefined.
   */
  type?: any

  /**
   * To detect `multiformats`. If a number, then the encoding is
   * assumed to have a {@link format} of 'view'.
   * @see https://github.com/multiformats/js-multiformats/blob/master/src/codecs/interface.ts
   */
  code?: any
}

/**
 * Names of built-in encodings.
 */
export type KnownEncodingName = 'utf8' | 'buffer' | 'view' | 'json' | 'hex' | 'base64'

/**
 * One of the supported encoding interfaces.
 */
export type MixedEncoding<TIn, TFormat, TOut> =
  IEncoding<TIn, TFormat, TOut> |
  IExternalEncoding<TIn, TFormat, TOut>

/**
 * Type utility to cast a built-in encoding identified by its name to an {@link Encoding}.
 */
export type KnownEncoding<N extends KnownEncodingName, TFormat>
  = Encoding<KnownEncodingInput<N>, TFormat, KnownEncodingOutput<N>>

/**
 * Type utility to get the input type of a built-in encoding identified by its name.
 */
export type KnownEncodingInput<N extends KnownEncodingName>
  = N extends 'utf8' ? string | Buffer | Uint8Array
    : N extends 'buffer' ? Buffer | Uint8Array | string
      : N extends 'view' ? Uint8Array | string
        : N extends 'json' ? any
          : N extends 'hex' ? Buffer | string
            : N extends 'base64' ? Buffer | string
              : never

/**
 * Type utility to get the output type of a built-in encoding identified by its name.
 */
export type KnownEncodingOutput<N extends KnownEncodingName>
  = N extends 'utf8' ? string
    : N extends 'buffer' ? Buffer
      : N extends 'view' ? Uint8Array
        : N extends 'json' ? any
          : N extends 'hex' ? string
            : N extends 'base64' ? string
              : never

/**
 * Type utility to use a {@link MixedEncoding} with an untyped format.
 */
export type PartialEncoding<TIn, TOut = TIn> = MixedEncoding<TIn, any, TOut>

/**
 * Type utility to use a {@link MixedEncoding} with an untyped format and output.
 * For when only the encoding side is needed.
 */
export type PartialEncoder<TIn> = MixedEncoding<TIn, any, any>

/**
 * Type utility to use a {@link MixedEncoding} with an untyped input and format.
 * For when only the decoding side is needed.
 */
export type PartialDecoder<TOut> = MixedEncoding<any, any, TOut>
