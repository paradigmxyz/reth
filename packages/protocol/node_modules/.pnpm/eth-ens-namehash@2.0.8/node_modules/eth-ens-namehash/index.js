var sha3 = require('js-sha3').keccak_256
var uts46 = require('idna-uts46-hx')

function namehash (inputName) {
  // Reject empty names:
  var node = ''
  for (var i = 0; i < 32; i++) {
    node += '00'
  }

  name = normalize(inputName)

  if (name) {
    var labels = name.split('.')

    for(var i = labels.length - 1; i >= 0; i--) {
      var labelSha = sha3(labels[i])
      node = sha3(new Buffer(node + labelSha, 'hex'))
    }
  }

  return '0x' + node
}

function normalize(name) {
  return name ? uts46.toUnicode(name, {useStd3ASCII: true, transitional: false}) : name
}

exports.hash = namehash
exports.normalize = normalize
