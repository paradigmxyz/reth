/* eslint-env mocha */

'use strict'

const assert = require('assert')
const encodeUtf8 = require('./')

const testCases = [
  'ﾟ･✿ヾ╲(｡◕‿◕｡)╱✿･ﾟ',
  '𝌆',
  '🐵 🙈 🙉 🙊',
  '💩',
  'åß∂ƒ©˙∆˚¬…æ',
  'Hello, World!',
  'Powerلُلُصّبُلُلصّبُررً ॣ ॣh ॣ ॣ冗',
  '𝕿𝖍𝖊 𝖖𝖚𝖎𝖈𝖐 𝖇𝖗𝖔𝖜𝖓 𝖋𝖔𝖝 𝖏𝖚𝖒𝖕𝖘 𝖔𝖛𝖊𝖗 𝖙𝖍𝖊 𝖑𝖆𝖟𝖞 𝖉𝖔𝖌',
  '사회과학원 어학연구소'
]

const badStrings = [
  {
    input: 'abc123',
    expected: [0x61, 0x62, 0x63, 0x31, 0x32, 0x33],
    name: 'Sanity check'
  },
  {
    input: '\uD800',
    expected: [0xef, 0xbf, 0xbd],
    name: 'Surrogate half (low)'
  },
  {
    input: '\uDC00',
    expected: [0xef, 0xbf, 0xbd],
    name: 'Surrogate half (high)'
  },
  {
    input: 'abc\uD800123',
    expected: [0x61, 0x62, 0x63, 0xef, 0xbf, 0xbd, 0x31, 0x32, 0x33],
    name: 'Surrogate half (low), in a string'
  },
  {
    input: 'abc\uDC00123',
    expected: [0x61, 0x62, 0x63, 0xef, 0xbf, 0xbd, 0x31, 0x32, 0x33],
    name: 'Surrogate half (high), in a string'
  },
  {
    input: '\uDC00\uD800',
    expected: [0xef, 0xbf, 0xbd, 0xef, 0xbf, 0xbd],
    name: 'Wrong order'
  }
]

describe('encode-utf8', () => {
  describe('test strings', () => {
    for (const input of testCases) {
      it(`should encode "${input}"`, () => {
        const actual = Buffer.from(encodeUtf8(input))
        const expected = Buffer.from(input, 'utf8')

        assert.ok(actual.equals(expected))
      })
    }
  })

  describe('web platform test', () => {
    for (const testCase of badStrings) {
      it(testCase.name, () => {
        const actual = Array.from(new Uint8Array(encodeUtf8(testCase.input)))

        assert.deepStrictEqual(actual, testCase.expected)
      })
    }
  })
})
