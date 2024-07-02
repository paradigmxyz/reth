'use strict'

const { AbstractIterator } = require('abstract-level')
const createKeyRange = require('./util/key-range')
const deserialize = require('./util/deserialize')

const kCache = Symbol('cache')
const kFinished = Symbol('finished')
const kOptions = Symbol('options')
const kCurrentOptions = Symbol('currentOptions')
const kPosition = Symbol('position')
const kLocation = Symbol('location')
const kFirst = Symbol('first')
const emptyOptions = {}

class Iterator extends AbstractIterator {
  constructor (db, location, options) {
    super(db, options)

    this[kCache] = []
    this[kFinished] = this.limit === 0
    this[kOptions] = options
    this[kCurrentOptions] = { ...options }
    this[kPosition] = undefined
    this[kLocation] = location
    this[kFirst] = true
  }

  // Note: if called by _all() then size can be Infinity. This is an internal
  // detail; by design AbstractIterator.nextv() does not support Infinity.
  _nextv (size, options, callback) {
    this[kFirst] = false

    if (this[kFinished]) {
      return this.nextTick(callback, null, [])
    } else if (this[kCache].length > 0) {
      // TODO: mixing next and nextv is not covered by test suite
      size = Math.min(size, this[kCache].length)
      return this.nextTick(callback, null, this[kCache].splice(0, size))
    }

    // Adjust range by what we already visited
    if (this[kPosition] !== undefined) {
      if (this[kOptions].reverse) {
        this[kCurrentOptions].lt = this[kPosition]
        this[kCurrentOptions].lte = undefined
      } else {
        this[kCurrentOptions].gt = this[kPosition]
        this[kCurrentOptions].gte = undefined
      }
    }

    let keyRange

    try {
      keyRange = createKeyRange(this[kCurrentOptions])
    } catch (_) {
      // The lower key is greater than the upper key.
      // IndexedDB throws an error, but we'll just return 0 results.
      this[kFinished] = true
      return this.nextTick(callback, null, [])
    }

    const transaction = this.db.db.transaction([this[kLocation]], 'readonly')
    const store = transaction.objectStore(this[kLocation])
    const entries = []

    if (!this[kOptions].reverse) {
      let keys
      let values

      const complete = () => {
        // Wait for both requests to complete
        if (keys === undefined || values === undefined) return

        const length = Math.max(keys.length, values.length)

        if (length === 0 || size === Infinity) {
          this[kFinished] = true
        } else {
          this[kPosition] = keys[length - 1]
        }

        // Resize
        entries.length = length

        // Merge keys and values
        for (let i = 0; i < length; i++) {
          const key = keys[i]
          const value = values[i]

          entries[i] = [
            this[kOptions].keys && key !== undefined ? deserialize(key) : undefined,
            this[kOptions].values && value !== undefined ? deserialize(value) : undefined
          ]
        }

        maybeCommit(transaction)
      }

      // If keys were not requested and size is Infinity, we don't have to keep
      // track of position and can thus skip getting keys.
      if (this[kOptions].keys || size < Infinity) {
        store.getAllKeys(keyRange, size < Infinity ? size : undefined).onsuccess = (ev) => {
          keys = ev.target.result
          complete()
        }
      } else {
        keys = []
        this.nextTick(complete)
      }

      if (this[kOptions].values) {
        store.getAll(keyRange, size < Infinity ? size : undefined).onsuccess = (ev) => {
          values = ev.target.result
          complete()
        }
      } else {
        values = []
        this.nextTick(complete)
      }
    } else {
      // Can't use getAll() in reverse, so use a slower cursor that yields one item at a time
      // TODO: test if all target browsers support openKeyCursor
      const method = !this[kOptions].values && store.openKeyCursor ? 'openKeyCursor' : 'openCursor'

      store[method](keyRange, 'prev').onsuccess = (ev) => {
        const cursor = ev.target.result

        if (cursor) {
          const { key, value } = cursor
          this[kPosition] = key

          entries.push([
            this[kOptions].keys && key !== undefined ? deserialize(key) : undefined,
            this[kOptions].values && value !== undefined ? deserialize(value) : undefined
          ])

          if (entries.length < size) {
            cursor.continue()
          } else {
            maybeCommit(transaction)
          }
        } else {
          this[kFinished] = true
        }
      }
    }

    // If an error occurs (on the request), the transaction will abort.
    transaction.onabort = () => {
      callback(transaction.error || new Error('aborted by user'))
      callback = null
    }

    transaction.oncomplete = () => {
      callback(null, entries)
      callback = null
    }
  }

  _next (callback) {
    if (this[kCache].length > 0) {
      const [key, value] = this[kCache].shift()
      this.nextTick(callback, null, key, value)
    } else if (this[kFinished]) {
      this.nextTick(callback)
    } else {
      let size = Math.min(100, this.limit - this.count)

      if (this[kFirst]) {
        // It's common to only want one entry initially or after a seek()
        this[kFirst] = false
        size = 1
      }

      this._nextv(size, emptyOptions, (err, entries) => {
        if (err) return callback(err)
        this[kCache] = entries
        this._next(callback)
      })
    }
  }

  _all (options, callback) {
    this[kFirst] = false

    // TODO: mixing next and all is not covered by test suite
    const cache = this[kCache].splice(0, this[kCache].length)
    const size = this.limit - this.count - cache.length

    if (size <= 0) {
      return this.nextTick(callback, null, cache)
    }

    this._nextv(size, emptyOptions, (err, entries) => {
      if (err) return callback(err)
      if (cache.length > 0) entries = cache.concat(entries)
      callback(null, entries)
    })
  }

  _seek (target, options) {
    this[kFirst] = true
    this[kCache] = []
    this[kFinished] = false
    this[kPosition] = undefined

    // TODO: not covered by test suite
    this[kCurrentOptions] = { ...this[kOptions] }

    let keyRange

    try {
      keyRange = createKeyRange(this[kOptions])
    } catch (_) {
      this[kFinished] = true
      return
    }

    if (keyRange !== null && !keyRange.includes(target)) {
      this[kFinished] = true
    } else if (this[kOptions].reverse) {
      this[kCurrentOptions].lte = target
    } else {
      this[kCurrentOptions].gte = target
    }
  }
}

exports.Iterator = Iterator

function maybeCommit (transaction) {
  // Commit (meaning close) now instead of waiting for auto-commit
  if (typeof transaction.commit === 'function') {
    transaction.commit()
  }
}
