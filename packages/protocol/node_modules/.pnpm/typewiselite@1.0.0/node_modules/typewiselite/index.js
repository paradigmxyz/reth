function inequality (a, b) {
  return a === b ? 0 : a < b ? -1 : 1
}

function buffercmp (a, b) {
  var l = Math.min(a.length, b.length)
  for(var i = 0; i < l; i++)
    if(a[i] !== b[i]) return inequality(a[i], b[i])
  return a.length - b.length
}

function arraycmp (a, b) {
  var l = Math.min(a.length, b.length)
  for(var i = 0; i < l; i++) {
    var c = compare(a[i], b[i])
    if(c) return c
  }

  return inequality(a.length, b.length)

}

var comparators = [
  inequality, // null
  inequality, // boolean
  inequality, // number
  buffercmp,  // buffer
  inequality, // string
  ,           // object
  arraycmp,   // array
  inequality  // undefined
]

function getType (v) {
  if(v === null)         return 0
  var t = typeof v
  if(t === 'boolean')    return 1
  if(t === 'number')     return 2
  if(Buffer.isBuffer(v)) return 3
  if(Array.isArray(v))   return 6
  if(t === 'string')     return 4
  if(t === 'undefined')  return 7

  throw new Error('comparing objects or functions is not supported')
  //                     return 5
}

function compare (a, b) {
  var t = getType(a)
  return inequality(t, getType(b)) || comparators[t](a, b)
}

module.exports = compare

module.exports.equal = function (a, b) {
  return compare(a, b) === 0
}
