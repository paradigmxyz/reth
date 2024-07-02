module.exports = {
  encode: function (e) {
    var s = '';
    var prefix = e[0].slice()
    while(prefix.length) {
      s += '\xff' + prefix.shift().toString() + '\xff'
    }
    return s + (e[1] || '').toString()
  },
  decode: function (e) {
    var k = e.toString().split('\xff').filter(Boolean)
    var j = k.pop()
    return [k, j]
  },
  lowerBound: '\x00',
  upperBound: '\xff'
}

