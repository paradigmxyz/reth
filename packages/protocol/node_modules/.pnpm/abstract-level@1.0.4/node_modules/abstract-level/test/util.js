'use strict'

const ModuleError = require('module-error')
const { AbstractLevel, AbstractChainedBatch } = require('..')
const { AbstractIterator, AbstractKeyIterator, AbstractValueIterator } = require('..')

const spies = []

exports.verifyNotFoundError = function (err) {
  return err.code === 'LEVEL_NOT_FOUND' && err.notFound === true && err.status === 404
}

exports.illegalKeys = [
  { name: 'null key', key: null },
  { name: 'undefined key', key: undefined }
]

exports.illegalValues = [
  { name: 'null key', value: null },
  { name: 'undefined value', value: undefined }
]

/**
 * Wrap a callback to check that it's called asynchronously. Must be
 * combined with a `ctx()`, `with()` or `end()` call.
 *
 * @param {function} cb Callback to check.
 * @param {string} name Optional callback name to use in assertion messages.
 * @returns {function} Wrapped callback.
 */
exports.assertAsync = function (cb, name) {
  const spy = {
    called: false,
    name: name || cb.name || 'anonymous'
  }

  spies.push(spy)

  return function (...args) {
    spy.called = true
    return cb.apply(this, args)
  }
}

/**
 * Verify that callbacks wrapped with `assertAsync()` were not yet called.
 * @param {import('tape').Test} t Tape test object.
 */
exports.assertAsync.end = function (t) {
  for (const { called, name } of spies.splice(0, spies.length)) {
    t.is(called, false, `callback (${name}) is asynchronous`)
  }
}

/**
 * Wrap a test function to verify `assertAsync()` spies at the end.
 * @param {import('tape').TestCase} test Test function to be passed to `tape()`.
 * @returns {import('tape').TestCase} Wrapped test function.
 */
exports.assertAsync.ctx = function (test) {
  return function (...args) {
    const ret = test.call(this, ...args)
    exports.assertAsync.end(args[0])
    return ret
  }
}

/**
 * Wrap an arbitrary callback to verify `assertAsync()` spies at the end.
 * @param {import('tape').Test} t Tape test object.
 * @param {function} cb Callback to wrap.
 * @returns {function} Wrapped callback.
 */
exports.assertAsync.with = function (t, cb) {
  return function (...args) {
    const ret = cb.call(this, ...args)
    exports.assertAsync.end(t)
    return ret
  }
}

exports.mockLevel = function (methods, ...args) {
  class TestLevel extends AbstractLevel {}
  for (const k in methods) TestLevel.prototype[k] = methods[k]
  if (!args.length) args = [{ encodings: { utf8: true } }]
  return new TestLevel(...args)
}

exports.mockIterator = function (db, options, methods, ...args) {
  class TestIterator extends AbstractIterator {}
  for (const k in methods) TestIterator.prototype[k] = methods[k]
  return new TestIterator(db, options, ...args)
}

exports.mockChainedBatch = function (db, methods, ...args) {
  class TestBatch extends AbstractChainedBatch {}
  for (const k in methods) TestBatch.prototype[k] = methods[k]
  return new TestBatch(db, ...args)
}

// Mock encoding where null and undefined are significant types
exports.nullishEncoding = {
  name: 'nullish',
  format: 'utf8',
  encode (v) {
    return v === null ? '\x00' : v === undefined ? '\xff' : String(v)
  },
  decode (v) {
    return v === '\x00' ? null : v === '\xff' ? undefined : v
  }
}

const kEntries = Symbol('entries')
const kPosition = Symbol('position')
const kOptions = Symbol('options')

/**
 * A minimal and non-optimized implementation for use in tests. Only supports utf8.
 * Don't use this as a reference implementation.
 */
class MinimalLevel extends AbstractLevel {
  constructor (options) {
    super({ encodings: { utf8: true }, seek: true }, options)
    this[kEntries] = new Map()
  }

  _put (key, value, options, callback) {
    this[kEntries].set(key, value)
    this.nextTick(callback)
  }

  _get (key, options, callback) {
    const value = this[kEntries].get(key)

    if (value === undefined) {
      return this.nextTick(callback, new ModuleError(`Key ${key} was not found`, {
        code: 'LEVEL_NOT_FOUND'
      }))
    }

    this.nextTick(callback, null, value)
  }

  _getMany (keys, options, callback) {
    const values = keys.map(k => this[kEntries].get(k))
    this.nextTick(callback, null, values)
  }

  _del (key, options, callback) {
    this[kEntries].delete(key)
    this.nextTick(callback)
  }

  _clear (options, callback) {
    for (const [k] of sliceEntries(this[kEntries], options, true)) {
      this[kEntries].delete(k)
    }

    this.nextTick(callback)
  }

  _batch (operations, options, callback) {
    const entries = new Map(this[kEntries])

    for (const op of operations) {
      if (op.type === 'put') entries.set(op.key, op.value)
      else entries.delete(op.key)
    }

    this[kEntries] = entries
    this.nextTick(callback)
  }

  _iterator (options) {
    return new MinimalIterator(this, options)
  }

  _keys (options) {
    return new MinimalKeyIterator(this, options)
  }

  _values (options) {
    return new MinimalValueIterator(this, options)
  }
}

class MinimalIterator extends AbstractIterator {
  constructor (db, options) {
    super(db, options)
    this[kEntries] = sliceEntries(db[kEntries], options, false)
    this[kOptions] = options
    this[kPosition] = 0
  }
}

class MinimalKeyIterator extends AbstractKeyIterator {
  constructor (db, options) {
    super(db, options)
    this[kEntries] = sliceEntries(db[kEntries], options, false)
    this[kOptions] = options
    this[kPosition] = 0
  }
}

class MinimalValueIterator extends AbstractValueIterator {
  constructor (db, options) {
    super(db, options)
    this[kEntries] = sliceEntries(db[kEntries], options, false)
    this[kOptions] = options
    this[kPosition] = 0
  }
}

for (const Ctor of [MinimalIterator, MinimalKeyIterator, MinimalValueIterator]) {
  const mapEntry = Ctor === MinimalIterator ? e => e : Ctor === MinimalKeyIterator ? e => e[0] : e => e[1]

  Ctor.prototype._next = function (callback) {
    const entry = this[kEntries][this[kPosition]++]
    if (entry === undefined) return this.nextTick(callback)
    if (Ctor === MinimalIterator) this.nextTick(callback, null, entry[0], entry[1])
    else this.nextTick(callback, null, mapEntry(entry))
  }

  Ctor.prototype._nextv = function (size, options, callback) {
    const entries = this[kEntries].slice(this[kPosition], this[kPosition] + size)
    this[kPosition] += entries.length
    this.nextTick(callback, null, entries.map(mapEntry))
  }

  Ctor.prototype._all = function (options, callback) {
    const end = this.limit - this.count + this[kPosition]
    const entries = this[kEntries].slice(this[kPosition], end)
    this[kPosition] = this[kEntries].length
    this.nextTick(callback, null, entries.map(mapEntry))
  }

  Ctor.prototype._seek = function (target, options) {
    this[kPosition] = this[kEntries].length

    if (!outOfRange(target, this[kOptions])) {
      // Don't care about performance here
      for (let i = 0; i < this[kPosition]; i++) {
        const key = this[kEntries][i][0]

        if (this[kOptions].reverse ? key <= target : key >= target) {
          this[kPosition] = i
        }
      }
    }
  }
}

const outOfRange = function (target, options) {
  if ('gte' in options) {
    if (target < options.gte) return true
  } else if ('gt' in options) {
    if (target <= options.gt) return true
  }

  if ('lte' in options) {
    if (target > options.lte) return true
  } else if ('lt' in options) {
    if (target >= options.lt) return true
  }

  return false
}

const sliceEntries = function (entries, options, applyLimit) {
  entries = Array.from(entries)
    .filter((e) => !outOfRange(e[0], options))
    .sort((a, b) => a[0] > b[0] ? 1 : a[0] < b[0] ? -1 : 0)

  if (options.reverse) entries.reverse()
  if (applyLimit && options.limit !== -1) entries = entries.slice(0, options.limit)

  return entries
}

exports.MinimalLevel = MinimalLevel
