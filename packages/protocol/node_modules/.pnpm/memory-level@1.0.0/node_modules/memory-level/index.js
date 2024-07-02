'use strict'

const {
  AbstractLevel,
  AbstractIterator,
  AbstractKeyIterator,
  AbstractValueIterator
} = require('abstract-level')

const ModuleError = require('module-error')
const createRBT = require('functional-red-black-tree')

const rangeOptions = new Set(['gt', 'gte', 'lt', 'lte'])
const kNone = Symbol('none')
const kTree = Symbol('tree')
const kIterator = Symbol('iterator')
const kLowerBound = Symbol('lowerBound')
const kUpperBound = Symbol('upperBound')
const kOutOfRange = Symbol('outOfRange')
const kReverse = Symbol('reverse')
const kOptions = Symbol('options')
const kTest = Symbol('test')
const kAdvance = Symbol('advance')
const kInit = Symbol('init')

function compare (a, b) {
  // Only relevant when storeEncoding is 'utf8',
  // which guarantees that b is also a string.
  if (typeof a === 'string') {
    return a < b ? -1 : a > b ? 1 : 0
  }

  const length = Math.min(a.byteLength, b.byteLength)

  for (let i = 0; i < length; i++) {
    const cmp = a[i] - b[i]
    if (cmp !== 0) return cmp
  }

  return a.byteLength - b.byteLength
}

function gt (value) {
  return compare(value, this[kUpperBound]) > 0
}

function gte (value) {
  return compare(value, this[kUpperBound]) >= 0
}

function lt (value) {
  return compare(value, this[kUpperBound]) < 0
}

function lte (value) {
  return compare(value, this[kUpperBound]) <= 0
}

class MemoryIterator extends AbstractIterator {
  constructor (db, options) {
    super(db, options)
    this[kInit](db[kTree], options)
  }

  _next (callback) {
    if (!this[kIterator].valid) return this.nextTick(callback)

    const key = this[kIterator].key
    const value = this[kIterator].value

    if (!this[kTest](key)) return this.nextTick(callback)

    this[kIterator][this[kAdvance]]()
    this.nextTick(callback, null, key, value)
  }

  _nextv (size, options, callback) {
    const it = this[kIterator]
    const entries = []

    while (it.valid && entries.length < size && this[kTest](it.key)) {
      entries.push([it.key, it.value])
      it[this[kAdvance]]()
    }

    this.nextTick(callback, null, entries)
  }

  _all (options, callback) {
    const size = this.limit - this.count
    const it = this[kIterator]
    const entries = []

    while (it.valid && entries.length < size && this[kTest](it.key)) {
      entries.push([it.key, it.value])
      it[this[kAdvance]]()
    }

    this.nextTick(callback, null, entries)
  }
}

class MemoryKeyIterator extends AbstractKeyIterator {
  constructor (db, options) {
    super(db, options)
    this[kInit](db[kTree], options)
  }

  _next (callback) {
    if (!this[kIterator].valid) return this.nextTick(callback)

    const key = this[kIterator].key
    if (!this[kTest](key)) return this.nextTick(callback)

    this[kIterator][this[kAdvance]]()
    this.nextTick(callback, null, key)
  }

  _nextv (size, options, callback) {
    const it = this[kIterator]
    const keys = []

    while (it.valid && keys.length < size && this[kTest](it.key)) {
      keys.push(it.key)
      it[this[kAdvance]]()
    }

    this.nextTick(callback, null, keys)
  }

  _all (options, callback) {
    const size = this.limit - this.count
    const it = this[kIterator]
    const keys = []

    while (it.valid && keys.length < size && this[kTest](it.key)) {
      keys.push(it.key)
      it[this[kAdvance]]()
    }

    this.nextTick(callback, null, keys)
  }
}

class MemoryValueIterator extends AbstractValueIterator {
  constructor (db, options) {
    super(db, options)
    this[kInit](db[kTree], options)
  }

  _next (callback) {
    if (!this[kIterator].valid) return this.nextTick(callback)

    const key = this[kIterator].key
    const value = this[kIterator].value

    if (!this[kTest](key)) return this.nextTick(callback)

    this[kIterator][this[kAdvance]]()
    this.nextTick(callback, null, value)
  }

  _nextv (size, options, callback) {
    const it = this[kIterator]
    const values = []

    while (it.valid && values.length < size && this[kTest](it.key)) {
      values.push(it.value)
      it[this[kAdvance]]()
    }

    this.nextTick(callback, null, values)
  }

  _all (options, callback) {
    const size = this.limit - this.count
    const it = this[kIterator]
    const values = []

    while (it.valid && values.length < size && this[kTest](it.key)) {
      values.push(it.value)
      it[this[kAdvance]]()
    }

    this.nextTick(callback, null, values)
  }
}

for (const Ctor of [MemoryIterator, MemoryKeyIterator, MemoryValueIterator]) {
  Ctor.prototype[kInit] = function (tree, options) {
    this[kReverse] = options.reverse
    this[kOptions] = options

    if (!this[kReverse]) {
      this[kAdvance] = 'next'
      this[kLowerBound] = 'gte' in options ? options.gte : 'gt' in options ? options.gt : kNone
      this[kUpperBound] = 'lte' in options ? options.lte : 'lt' in options ? options.lt : kNone

      if (this[kLowerBound] === kNone) {
        this[kIterator] = tree.begin
      } else if ('gte' in options) {
        this[kIterator] = tree.ge(this[kLowerBound])
      } else {
        this[kIterator] = tree.gt(this[kLowerBound])
      }

      if (this[kUpperBound] !== kNone) {
        this[kTest] = 'lte' in options ? lte : lt
      }
    } else {
      this[kAdvance] = 'prev'
      this[kLowerBound] = 'lte' in options ? options.lte : 'lt' in options ? options.lt : kNone
      this[kUpperBound] = 'gte' in options ? options.gte : 'gt' in options ? options.gt : kNone

      if (this[kLowerBound] === kNone) {
        this[kIterator] = tree.end
      } else if ('lte' in options) {
        this[kIterator] = tree.le(this[kLowerBound])
      } else {
        this[kIterator] = tree.lt(this[kLowerBound])
      }

      if (this[kUpperBound] !== kNone) {
        this[kTest] = 'gte' in options ? gte : gt
      }
    }
  }

  Ctor.prototype[kTest] = function () {
    return true
  }

  Ctor.prototype[kOutOfRange] = function (target) {
    if (!this[kTest](target)) {
      return true
    } else if (this[kLowerBound] === kNone) {
      return false
    } else if (!this[kReverse]) {
      if ('gte' in this[kOptions]) {
        return compare(target, this[kLowerBound]) < 0
      } else {
        return compare(target, this[kLowerBound]) <= 0
      }
    } else {
      if ('lte' in this[kOptions]) {
        return compare(target, this[kLowerBound]) > 0
      } else {
        return compare(target, this[kLowerBound]) >= 0
      }
    }
  }

  Ctor.prototype._seek = function (target, options) {
    if (this[kOutOfRange](target)) {
      this[kIterator] = this[kIterator].tree.end
      this[kIterator].next()
    } else if (this[kReverse]) {
      this[kIterator] = this[kIterator].tree.le(target)
    } else {
      this[kIterator] = this[kIterator].tree.ge(target)
    }
  }
}

class MemoryLevel extends AbstractLevel {
  constructor (location, options, _) {
    // Take a dummy location argument to align with other implementations
    if (typeof location === 'object' && location !== null) {
      options = location
    }

    // To help migrating from level-mem to abstract-level
    if (typeof location === 'function' || typeof options === 'function' || typeof _ === 'function') {
      throw new ModuleError('The levelup-style callback argument has been removed', {
        code: 'LEVEL_LEGACY'
      })
    }

    let { storeEncoding, ...forward } = options || {}
    storeEncoding = storeEncoding || 'buffer'

    // Our compare() function supports Buffer, Uint8Array and strings
    if (!['buffer', 'view', 'utf8'].includes(storeEncoding)) {
      throw new ModuleError("The storeEncoding option must be 'buffer', 'view' or 'utf8'", {
        code: 'LEVEL_ENCODING_NOT_SUPPORTED'
      })
    }

    super({
      seek: true,
      permanence: false,
      createIfMissing: false,
      errorIfExists: false,
      encodings: { [storeEncoding]: true }
    }, forward)

    this[kTree] = createRBT(compare)
  }

  _put (key, value, options, callback) {
    const it = this[kTree].find(key)

    if (it.valid) {
      this[kTree] = it.update(value)
    } else {
      this[kTree] = this[kTree].insert(key, value)
    }

    this.nextTick(callback)
  }

  _get (key, options, callback) {
    const value = this[kTree].get(key)

    if (typeof value === 'undefined') {
      // TODO: use error code (not urgent, abstract-level normalizes this)
      return this.nextTick(callback, new Error('NotFound'))
    }

    this.nextTick(callback, null, value)
  }

  _getMany (keys, options, callback) {
    this.nextTick(callback, null, keys.map(key => this[kTree].get(key)))
  }

  _del (key, options, callback) {
    this[kTree] = this[kTree].remove(key)
    this.nextTick(callback)
  }

  _batch (operations, options, callback) {
    let tree = this[kTree]

    for (const op of operations) {
      const key = op.key
      const it = tree.find(key)

      if (op.type === 'put') {
        tree = it.valid ? it.update(op.value) : tree.insert(key, op.value)
      } else {
        tree = it.remove()
      }
    }

    this[kTree] = tree
    this.nextTick(callback)
  }

  _clear (options, callback) {
    if (options.limit === -1 && !Object.keys(options).some(isRangeOption)) {
      // Delete everything by creating a new empty tree.
      this[kTree] = createRBT(compare)
      return this.nextTick(callback)
    }

    const iterator = this._keys({ ...options })
    const limit = iterator.limit

    let count = 0

    const loop = () => {
      // TODO: add option to control "batch size"
      for (let i = 0; i < 500; i++) {
        if (++count > limit) return callback()
        if (!iterator[kIterator].valid) return callback()
        if (!iterator[kTest](iterator[kIterator].key)) return callback()

        // Must also include changes made in parallel to clear()
        this[kTree] = this[kTree].remove(iterator[kIterator].key)
        iterator[kIterator][iterator[kAdvance]]()
      }

      // Some time to breathe
      this.nextTick(loop)
    }

    this.nextTick(loop)
  }

  _iterator (options) {
    return new MemoryIterator(this, options)
  }

  _keys (options) {
    return new MemoryKeyIterator(this, options)
  }

  _values (options) {
    return new MemoryValueIterator(this, options)
  }
}

exports.MemoryLevel = MemoryLevel

// Use setImmediate() in Node.js to allow IO in between our callbacks
if (typeof process !== 'undefined' && !process.browser && typeof global !== 'undefined' && typeof global.setImmediate === 'function') {
  const setImmediate = global.setImmediate

  // Automatically applies to iterators, sublevels and chained batches as well
  MemoryLevel.prototype.nextTick = function (fn, ...args) {
    if (args.length === 0) {
      setImmediate(fn)
    } else {
      setImmediate(() => fn(...args))
    }
  }
}

function isRangeOption (k) {
  return rangeOptions.has(k)
}
