const rlp = require('rlp')
const ethUtil = require('ethereumjs-util')

module.exports = TrieNode

function TrieNode (type, key, value) {
  if (Array.isArray(type)) {
    // parse raw node
    this.parseNode(type)
  } else {
    this.type = type
    if (type === 'branch') {
      var values = key
      this.raw = Array.apply(null, Array(17))
      if (values) {
        values.forEach(function (keyVal) {
          this.set.apply(this, keyVal)
        })
      }
    } else {
      this.raw = Array(2)
      this.setValue(value)
      this.setKey(key)
    }
  }
}

TrieNode.isRawNode = isRawNode
TrieNode.addHexPrefix = addHexPrefix
TrieNode.removeHexPrefix = removeHexPrefix
TrieNode.isTerminator = isTerminator
TrieNode.stringToNibbles = stringToNibbles
TrieNode.nibblesToBuffer = nibblesToBuffer
TrieNode.getNodeType = getNodeType

Object.defineProperty(TrieNode.prototype, 'value', {
  get: function () {
    return this.getValue()
  },
  set: function (v) {
    this.setValue(v)
  }
})

Object.defineProperty(TrieNode.prototype, 'key', {
  get: function () {
    return this.getKey()
  },
  set: function (k) {
    this.setKey(k)
  }
})

// parses a raw node
TrieNode.prototype.parseNode = function (rawNode) {
  this.raw = rawNode
  this.type = getNodeType(rawNode)
}

// sets the value of the node
TrieNode.prototype.setValue = function (key, value) {
  if (this.type !== 'branch') {
    this.raw[1] = key
  } else {
    if (arguments.length === 1) {
      value = key
      key = 16
    }
    this.raw[key] = value
  }
}

TrieNode.prototype.getValue = function (key) {
  if (this.type === 'branch') {
    if (arguments.length === 0) {
      key = 16
    }

    var val = this.raw[key]
    if (val !== null && val !== undefined && val.length !== 0) {
      return val
    }
  } else {
    return this.raw[1]
  }
}

TrieNode.prototype.setKey = function (key) {
  if (this.type !== 'branch') {
    if (Buffer.isBuffer(key)) {
      key = stringToNibbles(key)
    } else {
      key = key.slice(0) // copy the key
    }

    key = addHexPrefix(key, this.type === 'leaf')
    this.raw[0] = nibblesToBuffer(key)
  }
}

// returns the key as a nibble
TrieNode.prototype.getKey = function () {
  if (this.type !== 'branch') {
    var key = this.raw[0]
    key = removeHexPrefix(stringToNibbles(key))
    return (key)
  }
}

TrieNode.prototype.serialize = function () {
  return rlp.encode(this.raw)
}

TrieNode.prototype.hash = function () {
  return ethUtil.sha3(this.serialize())
}

TrieNode.prototype.toString = function () {
  var out = this.type
  out += ': ['
  this.raw.forEach(function (el) {
    if (Buffer.isBuffer(el)) {
      out += el.toString('hex') + ', '
    } else if (el) {
      out += 'object, '
    } else {
      out += 'empty, '
    }
  })
  out = out.slice(0, -2)
  out += ']'
  return out
}

TrieNode.prototype.getChildren = function () {
  var children = []
  switch (this.type) {
    case 'leaf':
      // no children
      break
    case 'extention':
      // one child
      children.push([this.key, this.getValue()])
      break
    case 'branch':
      for (var index = 0, end = 16; index < end; index++) {
        var value = this.getValue(index)
        if (value) {
          children.push([
            [index], value
          ])
        }
      }
      break
  }
  return children
}

/**
 * @param {Array} dataArr
 * @returns {Buffer} - returns buffer of encoded data
 * hexPrefix
 **/
function addHexPrefix (key, terminator) {
  // odd
  if (key.length % 2) {
    key.unshift(1)
  } else {
    // even
    key.unshift(0)
    key.unshift(0)
  }

  if (terminator) {
    key[0] += 2
  }

  return key
}

function removeHexPrefix (val) {
  if (val[0] % 2) {
    val = val.slice(1)
  } else {
    val = val.slice(2)
  }

  return val
}

/**
 * Determines if a key has Arnold Schwarzenegger in it.
 * @method isTerminator
 * @param {Array} key - an hexprefixed array of nibbles
 */
function isTerminator (key) {
  return key[0] > 1
}

/**
 * Converts a string OR a buffer to a nibble array.
 * @method stringToNibbles
 * @param {Buffer| String} key
 */
function stringToNibbles (key) {
  var bkey = new Buffer(key)
  var nibbles = []

  for (var i = 0; i < bkey.length; i++) {
    var q = i * 2
    nibbles[q] = bkey[i] >> 4
    ++q
    nibbles[q] = bkey[i] % 16
  }
  return nibbles
}

/**
 * Converts a nibble array into a buffer.
 * @method nibblesToBuffer
 * @param arr
 */
function nibblesToBuffer (arr) {
  var buf = new Buffer(arr.length / 2)
  for (var i = 0; i < buf.length; i++) {
    var q = i * 2
    buf[i] = (arr[q] << 4) + arr[++q]
  }
  return buf
}

/**
 * Determines the node type.
 * @returns {String} - the node type
 *   - leaf - if the node is a leaf
 *   - branch - if the node is a branch
 *   - extention - if the node is an extention
 *   - unknown - if something else got borked
 */
function getNodeType (node) {
  if (node.length === 17) {
    return 'branch'
  } else if (node.length === 2) {
    var key = stringToNibbles(node[0])
    if (isTerminator(key)) {
      return 'leaf'
    }

    return 'extention'
  }
}

function isRawNode (node) {
  return Array.isArray(node) && !Buffer.isBuffer(node)
}
