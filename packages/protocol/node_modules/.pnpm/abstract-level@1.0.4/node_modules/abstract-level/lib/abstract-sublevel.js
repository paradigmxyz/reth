'use strict'

const ModuleError = require('module-error')
const { Buffer } = require('buffer') || {}
const {
  AbstractSublevelIterator,
  AbstractSublevelKeyIterator,
  AbstractSublevelValueIterator
} = require('./abstract-sublevel-iterator')

const kPrefix = Symbol('prefix')
const kUpperBound = Symbol('upperBound')
const kPrefixRange = Symbol('prefixRange')
const kParent = Symbol('parent')
const kUnfix = Symbol('unfix')

const textEncoder = new TextEncoder()
const defaults = { separator: '!' }

// Wrapped to avoid circular dependency
module.exports = function ({ AbstractLevel }) {
  class AbstractSublevel extends AbstractLevel {
    static defaults (options) {
      // To help migrating from subleveldown to abstract-level
      if (typeof options === 'string') {
        throw new ModuleError('The subleveldown string shorthand for { separator } has been removed', {
          code: 'LEVEL_LEGACY'
        })
      } else if (options && options.open) {
        throw new ModuleError('The subleveldown open option has been removed', {
          code: 'LEVEL_LEGACY'
        })
      }

      if (options == null) {
        return defaults
      } else if (!options.separator) {
        return { ...options, separator: '!' }
      } else {
        return options
      }
    }

    // TODO: add autoClose option, which if true, does parent.attachResource(this)
    constructor (db, name, options) {
      // Don't forward AbstractSublevel options to AbstractLevel
      const { separator, manifest, ...forward } = AbstractSublevel.defaults(options)
      name = trim(name, separator)

      // Reserve one character between separator and name to give us an upper bound
      const reserved = separator.charCodeAt(0) + 1
      const parent = db[kParent] || db

      // Keys should sort like ['!a!', '!a!!a!', '!a"', '!aa!', '!b!'].
      // Use ASCII for consistent length between string, Buffer and Uint8Array
      if (!textEncoder.encode(name).every(x => x > reserved && x < 127)) {
        throw new ModuleError(`Prefix must use bytes > ${reserved} < ${127}`, {
          code: 'LEVEL_INVALID_PREFIX'
        })
      }

      super(mergeManifests(parent, manifest), forward)

      const prefix = (db.prefix || '') + separator + name + separator
      const upperBound = prefix.slice(0, -1) + String.fromCharCode(reserved)

      this[kParent] = parent
      this[kPrefix] = new MultiFormat(prefix)
      this[kUpperBound] = new MultiFormat(upperBound)
      this[kUnfix] = new Unfixer()

      this.nextTick = parent.nextTick
    }

    prefixKey (key, keyFormat) {
      if (keyFormat === 'utf8') {
        return this[kPrefix].utf8 + key
      } else if (key.byteLength === 0) {
        // Fast path for empty key (no copy)
        return this[kPrefix][keyFormat]
      } else if (keyFormat === 'view') {
        const view = this[kPrefix].view
        const result = new Uint8Array(view.byteLength + key.byteLength)

        result.set(view, 0)
        result.set(key, view.byteLength)

        return result
      } else {
        const buffer = this[kPrefix].buffer
        return Buffer.concat([buffer, key], buffer.byteLength + key.byteLength)
      }
    }

    // Not exposed for now.
    [kPrefixRange] (range, keyFormat) {
      if (range.gte !== undefined) {
        range.gte = this.prefixKey(range.gte, keyFormat)
      } else if (range.gt !== undefined) {
        range.gt = this.prefixKey(range.gt, keyFormat)
      } else {
        range.gte = this[kPrefix][keyFormat]
      }

      if (range.lte !== undefined) {
        range.lte = this.prefixKey(range.lte, keyFormat)
      } else if (range.lt !== undefined) {
        range.lt = this.prefixKey(range.lt, keyFormat)
      } else {
        range.lte = this[kUpperBound][keyFormat]
      }
    }

    get prefix () {
      return this[kPrefix].utf8
    }

    get db () {
      return this[kParent]
    }

    _open (options, callback) {
      // The parent db must open itself or be (re)opened by the user because
      // a sublevel should not initiate state changes on the rest of the db.
      this[kParent].open({ passive: true }, callback)
    }

    _put (key, value, options, callback) {
      this[kParent].put(key, value, options, callback)
    }

    _get (key, options, callback) {
      this[kParent].get(key, options, callback)
    }

    _getMany (keys, options, callback) {
      this[kParent].getMany(keys, options, callback)
    }

    _del (key, options, callback) {
      this[kParent].del(key, options, callback)
    }

    _batch (operations, options, callback) {
      this[kParent].batch(operations, options, callback)
    }

    _clear (options, callback) {
      // TODO (refactor): move to AbstractLevel
      this[kPrefixRange](options, options.keyEncoding)
      this[kParent].clear(options, callback)
    }

    _iterator (options) {
      // TODO (refactor): move to AbstractLevel
      this[kPrefixRange](options, options.keyEncoding)
      const iterator = this[kParent].iterator(options)
      const unfix = this[kUnfix].get(this[kPrefix].utf8.length, options.keyEncoding)
      return new AbstractSublevelIterator(this, options, iterator, unfix)
    }

    _keys (options) {
      this[kPrefixRange](options, options.keyEncoding)
      const iterator = this[kParent].keys(options)
      const unfix = this[kUnfix].get(this[kPrefix].utf8.length, options.keyEncoding)
      return new AbstractSublevelKeyIterator(this, options, iterator, unfix)
    }

    _values (options) {
      this[kPrefixRange](options, options.keyEncoding)
      const iterator = this[kParent].values(options)
      return new AbstractSublevelValueIterator(this, options, iterator)
    }
  }

  return { AbstractSublevel }
}

const mergeManifests = function (parent, manifest) {
  return {
    // Inherit manifest of parent db
    ...parent.supports,

    // Disable unsupported features
    createIfMissing: false,
    errorIfExists: false,

    // Unset additional events because we're not forwarding them
    events: {},

    // Unset additional methods (like approximateSize) which we can't support here unless
    // the AbstractSublevel class is overridden by an implementation of `abstract-level`.
    additionalMethods: {},

    // Inherit manifest of custom AbstractSublevel subclass. Such a class is not
    // allowed to override encodings.
    ...manifest,

    encodings: {
      utf8: supportsEncoding(parent, 'utf8'),
      buffer: supportsEncoding(parent, 'buffer'),
      view: supportsEncoding(parent, 'view')
    }
  }
}

const supportsEncoding = function (parent, encoding) {
  // Prefer a non-transcoded encoding for optimal performance
  return parent.supports.encodings[encoding]
    ? parent.keyEncoding(encoding).name === encoding
    : false
}

class MultiFormat {
  constructor (key) {
    this.utf8 = key
    this.view = textEncoder.encode(key)
    this.buffer = Buffer ? Buffer.from(this.view.buffer, 0, this.view.byteLength) : {}
  }
}

class Unfixer {
  constructor () {
    this.cache = new Map()
  }

  get (prefixLength, keyFormat) {
    let unfix = this.cache.get(keyFormat)

    if (unfix === undefined) {
      if (keyFormat === 'view') {
        unfix = function (prefixLength, key) {
          // Avoid Uint8Array#slice() because it copies
          return key.subarray(prefixLength)
        }.bind(null, prefixLength)
      } else {
        unfix = function (prefixLength, key) {
          // Avoid Buffer#subarray() because it's slow
          return key.slice(prefixLength)
        }.bind(null, prefixLength)
      }

      this.cache.set(keyFormat, unfix)
    }

    return unfix
  }
}

const trim = function (str, char) {
  let start = 0
  let end = str.length

  while (start < end && str[start] === char) start++
  while (end > start && str[end - 1] === char) end--

  return str.slice(start, end)
}
