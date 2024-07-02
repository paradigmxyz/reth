import { KnownEncodingInput, KnownEncodingOutput } from './encoding'
import { BufferFormat, ViewFormat, UTF8Format } from './formats'

export const utf8: UTF8Format<KnownEncodingInput<'utf8'>, KnownEncodingOutput<'utf8'>>
export const json: UTF8Format<KnownEncodingInput<'json'>, KnownEncodingOutput<'json'>>
export const buffer: BufferFormat<KnownEncodingInput<'buffer'>, KnownEncodingOutput<'buffer'>>
export const view: ViewFormat<KnownEncodingInput<'view'>, KnownEncodingOutput<'view'>>
export const hex: BufferFormat<KnownEncodingInput<'hex'>, KnownEncodingOutput<'hex'>>
export const base64: BufferFormat<KnownEncodingInput<'base64'>, KnownEncodingOutput<'base64'>>
