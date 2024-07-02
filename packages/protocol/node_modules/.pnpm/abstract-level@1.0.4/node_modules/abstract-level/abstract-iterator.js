'use strict'

const { fromCallback } = require('catering')
const ModuleError = require('module-error')
const { getOptions, getCallback } = require('./lib/common')

const kPromise = Symbol('promise')
const kCallback = Symbol('callback')
const kWorking = Symbol('working')
const kHandleOne = Symbol('handleOne')
const kHandleMany = Symbol('handleMany')
const kAutoClose = Symbol('autoClose')
const kFinishWork = Symbol('finishWork')
const kReturnMany = Symbol('returnMany')
const kClosing = Symbol('closing')
const kHandleClose = Symbol('handleClose')
const kClosed = Symbol('closed')
const kCloseCallbacks = Symbol('closeCallbacks')
const kKeyEncoding = Symbol('keyEncoding')
const kValueEncoding = Symbol('valueEncoding')
const kAbortOnClose = Symbol('abortOnClose')
const kLegacy = Symbol('legacy')
const kKeys = Symbol('keys')
const kValues = Symbol('values')
const kLimit = Symbol('limit')
const kCount = Symbol('count')

const emptyOptions = Object.freeze({})
const noop = () => {}
let warnedEnd = false

// This class is an internal utility for common functionality between AbstractIterator,
// AbstractKeyIterator and AbstractValueIterator. It's not exported.
class CommonIterator {
  constructor (db, options, legacy) {
    if (typeof db !== 'object' || db === null) {
      const hint = db === null ? 'null' : typeof db
      throw new TypeError(`The first argument must be an abstract-level database, received ${hint}`)
    }

    if (typeof options !== 'object' || options === null) {
      throw new TypeError('The second argument must be an options object')
    }

    this[kClosed] = false
    this[kCloseCallbacks] = []
    this[kWorking] = false
    this[kClosing] = false
    this[kAutoClose] = false
    this[kCallback] = null
    this[kHandleOne] = this[kHandleOne].bind(this)
    this[kHandleMany] = this[kHandleMany].bind(this)
    this[kHandleClose] = this[kHandleClose].bind(this)
    this[kKeyEncoding] = options[kKeyEncoding]
    this[kValueEncoding] = options[kValueEncoding]
    this[kLegacy] = legacy
    this[kLimit] = Number.isInteger(options.limit) && options.limit >= 0 ? options.limit : Infinity
    this[kCount] = 0

    // Undocumented option to abort pending work on close(). Used by the
    // many-level module as a temporary solution to a blocked close().
    // TODO (next major): consider making this the default behavior. Native
    // implementations should have their own logic to safely close iterators.
    this[kAbortOnClose] = !!options.abortOnClose

    this.db = db
    this.db.attachResource(this)
    this.nextTick = db.nextTick
  }

  get count () {
    return this[kCount]
  }

  get limit () {
    return this[kLimit]
  }

  next (callback) {
    let promise

    if (callback === undefined) {
      promise = new Promise((resolve, reject) => {
        callback = (err, key, value) => {
          if (err) reject(err)
          else if (!this[kLegacy]) resolve(key)
          else if (key === undefined && value === undefined) resolve()
          else resolve([key, value])
        }
      })
    } else if (typeof callback !== 'function') {
      throw new TypeError('Callback must be a function')
    }

    if (this[kClosing]) {
      this.nextTick(callback, new ModuleError('Iterator is not open: cannot call next() after close()', {
        code: 'LEVEL_ITERATOR_NOT_OPEN'
      }))
    } else if (this[kWorking]) {
      this.nextTick(callback, new ModuleError('Iterator is busy: cannot call next() until previous call has completed', {
        code: 'LEVEL_ITERATOR_BUSY'
      }))
    } else {
      this[kWorking] = true
      this[kCallback] = callback

      if (this[kCount] >= this[kLimit]) this.nextTick(this[kHandleOne], null)
      else this._next(this[kHandleOne])
    }

    return promise
  }

  _next (callback) {
    this.nextTick(callback)
  }

  nextv (size, options, callback) {
    callback = getCallback(options, callback)
    callback = fromCallback(callback, kPromise)
    options = getOptions(options, emptyOptions)

    if (!Number.isInteger(size)) {
      this.nextTick(callback, new TypeError("The first argument 'size' must be an integer"))
      return callback[kPromise]
    }

    if (this[kClosing]) {
      this.nextTick(callback, new ModuleError('Iterator is not open: cannot call nextv() after close()', {
        code: 'LEVEL_ITERATOR_NOT_OPEN'
      }))
    } else if (this[kWorking]) {
      this.nextTick(callback, new ModuleError('Iterator is busy: cannot call nextv() until previous call has completed', {
        code: 'LEVEL_ITERATOR_BUSY'
      }))
    } else {
      if (size < 1) size = 1
      if (this[kLimit] < Infinity) size = Math.min(size, this[kLimit] - this[kCount])

      this[kWorking] = true
      this[kCallback] = callback

      if (size <= 0) this.nextTick(this[kHandleMany], null, [])
      else this._nextv(size, options, this[kHandleMany])
    }

    return callback[kPromise]
  }

  _nextv (size, options, callback) {
    const acc = []
    const onnext = (err, key, value) => {
      if (err) {
        return callback(err)
      } else if (this[kLegacy] ? key === undefined && value === undefined : key === undefined) {
        return callback(null, acc)
      }

      acc.push(this[kLegacy] ? [key, value] : key)

      if (acc.length === size) {
        callback(null, acc)
      } else {
        this._next(onnext)
      }
    }

    this._next(onnext)
  }

  all (options, callback) {
    callback = getCallback(options, callback)
    callback = fromCallback(callback, kPromise)
    options = getOptions(options, emptyOptions)

    if (this[kClosing]) {
      this.nextTick(callback, new ModuleError('Iterator is not open: cannot call all() after close()', {
        code: 'LEVEL_ITERATOR_NOT_OPEN'
      }))
    } else if (this[kWorking]) {
      this.nextTick(callback, new ModuleError('Iterator is busy: cannot call all() until previous call has completed', {
        code: 'LEVEL_ITERATOR_BUSY'
      }))
    } else {
      this[kWorking] = true
      this[kCallback] = callback
      this[kAutoClose] = true

      if (this[kCount] >= this[kLimit]) this.nextTick(this[kHandleMany], null, [])
      else this._all(options, this[kHandleMany])
    }

    return callback[kPromise]
  }

  _all (options, callback) {
    // Must count here because we're directly calling _nextv()
    let count = this[kCount]
    const acc = []

    const nextv = () => {
      // Not configurable, because implementations should optimize _all().
      const size = this[kLimit] < Infinity ? Math.min(1e3, this[kLimit] - count) : 1e3

      if (size <= 0) {
        this.nextTick(callback, null, acc)
      } else {
        this._nextv(size, emptyOptions, onnextv)
      }
    }

    const onnextv = (err, items) => {
      if (err) {
        callback(err)
      } else if (items.length === 0) {
        callback(null, acc)
      } else {
        acc.push.apply(acc, items)
        count += items.length
        nextv()
      }
    }

    nextv()
  }

  [kFinishWork] () {
    const cb = this[kCallback]

    // Callback will be null if work was aborted on close
    if (this[kAbortOnClose] && cb === null) return noop

    this[kWorking] = false
    this[kCallback] = null

    if (this[kClosing]) this._close(this[kHandleClose])

    return cb
  }

  [kReturnMany] (cb, err, items) {
    if (this[kAutoClose]) {
      this.close(cb.bind(null, err, items))
    } else {
      cb(err, items)
    }
  }

  seek (target, options) {
    options = getOptions(options, emptyOptions)

    if (this[kClosing]) {
      // Don't throw here, to be kind to implementations that wrap
      // another db and don't necessarily control when the db is closed
    } else if (this[kWorking]) {
      throw new ModuleError('Iterator is busy: cannot call seek() until next() has completed', {
        code: 'LEVEL_ITERATOR_BUSY'
      })
    } else {
      const keyEncoding = this.db.keyEncoding(options.keyEncoding || this[kKeyEncoding])
      const keyFormat = keyEncoding.format

      if (options.keyEncoding !== keyFormat) {
        options = { ...options, keyEncoding: keyFormat }
      }

      const mapped = this.db.prefixKey(keyEncoding.encode(target), keyFormat)
      this._seek(mapped, options)
    }
  }

  _seek (target, options) {
    throw new ModuleError('Iterator does not support seek()', {
      code: 'LEVEL_NOT_SUPPORTED'
    })
  }

  close (callback) {
    callback = fromCallback(callback, kPromise)

    if (this[kClosed]) {
      this.nextTick(callback)
    } else if (this[kClosing]) {
      this[kCloseCallbacks].push(callback)
    } else {
      this[kClosing] = true
      this[kCloseCallbacks].push(callback)

      if (!this[kWorking]) {
        this._close(this[kHandleClose])
      } else if (this[kAbortOnClose]) {
        // Don't wait for work to finish. Subsequently ignore the result.
        const cb = this[kFinishWork]()

        cb(new ModuleError('Aborted on iterator close()', {
          code: 'LEVEL_ITERATOR_NOT_OPEN'
        }))
      }
    }

    return callback[kPromise]
  }

  _close (callback) {
    this.nextTick(callback)
  }

  [kHandleClose] () {
    this[kClosed] = true
    this.db.detachResource(this)

    const callbacks = this[kCloseCallbacks]
    this[kCloseCallbacks] = []

    for (const cb of callbacks) {
      cb()
    }
  }

  async * [Symbol.asyncIterator] () {
    try {
      let item

      while ((item = (await this.next())) !== undefined) {
        yield item
      }
    } finally {
      if (!this[kClosed]) await this.close()
    }
  }
}

// For backwards compatibility this class is not (yet) called AbstractEntryIterator.
class AbstractIterator extends CommonIterator {
  constructor (db, options) {
    super(db, options, true)
    this[kKeys] = options.keys !== false
    this[kValues] = options.values !== false
  }

  [kHandleOne] (err, key, value) {
    const cb = this[kFinishWork]()
    if (err) return cb(err)

    try {
      key = this[kKeys] && key !== undefined ? this[kKeyEncoding].decode(key) : undefined
      value = this[kValues] && value !== undefined ? this[kValueEncoding].decode(value) : undefined
    } catch (err) {
      return cb(new IteratorDecodeError('entry', err))
    }

    if (!(key === undefined && value === undefined)) {
      this[kCount]++
    }

    cb(null, key, value)
  }

  [kHandleMany] (err, entries) {
    const cb = this[kFinishWork]()
    if (err) return this[kReturnMany](cb, err)

    try {
      for (const entry of entries) {
        const key = entry[0]
        const value = entry[1]

        entry[0] = this[kKeys] && key !== undefined ? this[kKeyEncoding].decode(key) : undefined
        entry[1] = this[kValues] && value !== undefined ? this[kValueEncoding].decode(value) : undefined
      }
    } catch (err) {
      return this[kReturnMany](cb, new IteratorDecodeError('entries', err))
    }

    this[kCount] += entries.length
    this[kReturnMany](cb, null, entries)
  }

  end (callback) {
    if (!warnedEnd && typeof console !== 'undefined') {
      warnedEnd = true
      console.warn(new ModuleError(
        'The iterator.end() method was renamed to close() and end() is an alias that will be removed in a future version',
        { code: 'LEVEL_LEGACY' }
      ))
    }

    return this.close(callback)
  }
}

class AbstractKeyIterator extends CommonIterator {
  constructor (db, options) {
    super(db, options, false)
  }

  [kHandleOne] (err, key) {
    const cb = this[kFinishWork]()
    if (err) return cb(err)

    try {
      key = key !== undefined ? this[kKeyEncoding].decode(key) : undefined
    } catch (err) {
      return cb(new IteratorDecodeError('key', err))
    }

    if (key !== undefined) this[kCount]++
    cb(null, key)
  }

  [kHandleMany] (err, keys) {
    const cb = this[kFinishWork]()
    if (err) return this[kReturnMany](cb, err)

    try {
      for (let i = 0; i < keys.length; i++) {
        const key = keys[i]
        keys[i] = key !== undefined ? this[kKeyEncoding].decode(key) : undefined
      }
    } catch (err) {
      return this[kReturnMany](cb, new IteratorDecodeError('keys', err))
    }

    this[kCount] += keys.length
    this[kReturnMany](cb, null, keys)
  }
}

class AbstractValueIterator extends CommonIterator {
  constructor (db, options) {
    super(db, options, false)
  }

  [kHandleOne] (err, value) {
    const cb = this[kFinishWork]()
    if (err) return cb(err)

    try {
      value = value !== undefined ? this[kValueEncoding].decode(value) : undefined
    } catch (err) {
      return cb(new IteratorDecodeError('value', err))
    }

    if (value !== undefined) this[kCount]++
    cb(null, value)
  }

  [kHandleMany] (err, values) {
    const cb = this[kFinishWork]()
    if (err) return this[kReturnMany](cb, err)

    try {
      for (let i = 0; i < values.length; i++) {
        const value = values[i]
        values[i] = value !== undefined ? this[kValueEncoding].decode(value) : undefined
      }
    } catch (err) {
      return this[kReturnMany](cb, new IteratorDecodeError('values', err))
    }

    this[kCount] += values.length
    this[kReturnMany](cb, null, values)
  }
}

// Internal utility, not typed or exported
class IteratorDecodeError extends ModuleError {
  constructor (subject, cause) {
    super(`Iterator could not decode ${subject}`, {
      code: 'LEVEL_DECODE_ERROR',
      cause
    })
  }
}

// To help migrating to abstract-level
for (const k of ['_ended property', '_nexting property', '_end method']) {
  Object.defineProperty(AbstractIterator.prototype, k.split(' ')[0], {
    get () { throw new ModuleError(`The ${k} has been removed`, { code: 'LEVEL_LEGACY' }) },
    set () { throw new ModuleError(`The ${k} has been removed`, { code: 'LEVEL_LEGACY' }) }
  })
}

// Exposed so that AbstractLevel can set these options
AbstractIterator.keyEncoding = kKeyEncoding
AbstractIterator.valueEncoding = kValueEncoding

exports.AbstractIterator = AbstractIterator
exports.AbstractKeyIterator = AbstractKeyIterator
exports.AbstractValueIterator = AbstractValueIterator
