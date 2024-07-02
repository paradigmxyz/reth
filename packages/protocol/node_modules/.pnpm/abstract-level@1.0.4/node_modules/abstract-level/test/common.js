'use strict'

const kNone = Symbol('none')
const kProtected = Symbol('protected')

function testCommon (options) {
  const factory = options.factory
  const test = options.test

  if (typeof factory !== 'function') {
    throw new TypeError('factory must be a function')
  }

  if (typeof test !== 'function') {
    throw new TypeError('test must be a function')
  }

  if (options.legacyRange != null) {
    throw new Error('The legacyRange option has been removed')
  }

  let supports = kNone

  return protect(options, {
    test: test,
    factory: factory,
    internals: options.internals || {},

    // Expose manifest through testCommon to more easily skip tests based on
    // supported features. Use a getter to only create a db once. Implicitly
    // we also test that the manifest doesn't change after the db constructor.
    get supports () {
      if (supports === kNone) this.supports = this.factory().supports
      return supports
    },

    // Prefer assigning early via manifest-test unless test.only() is used
    // in which case we create the manifest on-demand. Copy it to be safe.
    set supports (value) {
      if (supports === kNone) supports = JSON.parse(JSON.stringify(value))
    }
  })
}

module.exports = testCommon

// To help migrating from abstract-leveldown.
// Throw if test suite options are used instead of db.supports
function protect (options, testCommon) {
  const legacyOptions = [
    ['createIfMissing', true],
    ['errorIfExists', true],
    ['snapshots', true],
    ['seek', true],
    ['encodings', true],
    ['deferredOpen', true],
    ['streams', true],
    ['clear', true],
    ['getMany', true],
    ['bufferKeys', false],
    ['serialize', false],
    ['idempotentOpen', false],
    ['passiveOpen', false],
    ['openCallback', false]
  ]

  Object.defineProperty(testCommon, kProtected, {
    value: true
  })

  for (const [k, exists] of legacyOptions) {
    const msg = exists ? 'has moved to db.supports' : 'has been removed'

    // Options may be a testCommon instance
    if (!options[kProtected] && k in options) {
      throw new Error(`The test suite option '${k}' ${msg}`)
    }

    Object.defineProperty(testCommon, k, {
      get () {
        throw new Error(`The test suite option '${k}' ${msg}`)
      },
      set () {
        throw new Error(`The test suite option '${k}' ${msg}`)
      }
    })
  }

  return testCommon
}
