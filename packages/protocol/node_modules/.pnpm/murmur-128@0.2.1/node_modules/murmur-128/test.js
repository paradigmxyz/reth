var murmur = require('./')
var assert = require('assert')
var isEqual = require('arraybuffer-equal')
var hexToArrayBuffer = require('hex-to-array-buffer')

var testCases = [
  ['00000000000000000000000000000000', ''],
  ['30ef026f687d0c55687d0c55687d0c55', 'test'],
  ['f8c3526fe5bfe31be9c5ca68e9c5ca68', 'linus'],
  ['66bb23b11925801d12d15c0c12d15c0c', 'murmur'],
  ['4af48c99d9316236e698caf103cbcb7d', 'veni, vidi, vici'],
  ['8cb43ccb21e8a3cdf3c2b6d43e071848', 'Caesar non supra grammaticos!'],
  ['86f9b66e00e56f6a5606f615f5bd8576', 'Node.js® is a JavaScript runtime built on Chrome\'s V8 JavaScript engine.'],
  ['2980047883618469c236fe507291e49a', 'Lorem ipsum dolor sit amet, consectetur adipiscing elit. Donec consectetur mollis orci a consectetur. Maecenas in ornare ligula, vel venenatis mauris.'],
  ['3b68a9a0405eac259528afd9f5dd0d89', 'I will not buy this record, it is scratched.'],
  ['d249810bb8cf51f25ac956670939ff17', 'I will not buy this tobaconnists, it is scratched.'],
  ['ae86a1e36cba69e134d98b6a9cfa683c', 'My hovercraft is full of eels.'],
  ['5fd816e678f6e7fe9961b4da0fb95b5b', 'My 🚀 is full of 🦎.'],
  ['50ba39bb7c45b2e4766d8e7304936db6', '吉 星 高 照']
]

testCases.forEach(function (testCase) {
  assert(isEqual(murmur(testCase[1]), hexToArrayBuffer(testCase[0])))
})
