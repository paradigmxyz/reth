const Trie = require('../index.js')
const describe = require('tape')

describe('kv stream test', function (tester) {
  var it = tester.test
  var trie = new Trie()
  var init = [{
    type: 'del',
    key: 'father'
  }, {
    type: 'put',
    key: 'name',
    value: 'Yuri Irsenovich Kim'
  }, {
    type: 'put',
    key: 'dob',
    value: '16 February 1941'
  }, {
    type: 'put',
    key: 'spouse',
    value: 'Kim Young-sook'
  }, {
    type: 'put',
    key: 'occupation',
    value: 'Clown'
  }, {
    type: 'put',
    key: 'nameads',
    value: 'Yuri Irsenovich Kim'
  }, {
    type: 'put',
    key: 'namfde',
    value: 'Yuri Irsenovich Kim'
  }, {
    type: 'put',
    key: 'namsse',
    value: 'Yuri Irsenovich Kim'
  }, {
    type: 'put',
    key: 'dofab',
    value: '16 February 1941'
  }, {
    type: 'put',
    key: 'spoudse',
    value: 'Kim Young-sook'
  }, {
    type: 'put',
    key: 'occupdsation',
    value: 'Clown'
  }, {
    type: 'put',
    key: 'dozzzb',
    value: '16 February 1941'
  }, {
    type: 'put',
    key: 'spouszze',
    value: 'Kim Young-sook'
  }, {
    type: 'put',
    key: 'occupatdfion',
    value: 'Clown'
  }, {
    type: 'put',
    key: 'dssob',
    value: '16 February 1941'
  }, {
    type: 'put',
    key: 'spossuse',
    value: 'Kim Young-sook'
  }, {
    type: 'put',
    key: 'occupssation',
    value: 'Clown'
  }]

  var valObj = {}
  init.forEach(function (i) {
    if (i.type === 'put') valObj[i.key] = i.value
  })

  it('should populate trie', function (t) {
    trie.batch(init, function () {
      t.end()
    })
  })

  it('should fetch all of the nodes', function (t) {
    var stream = trie.createReadStream()
    stream.on('data', function (d) {
      t.equal(valObj[d.key.toString()], d.value.toString())
      delete valObj[d.key.toString()]
    })
    stream.on('end', function () {
      var keys = Object.keys(valObj)
      t.equal(keys.length, 0)
      t.end()
    })
  })
})

describe('db stream test', function (tester) {
  var it = tester.test
  var trie = new Trie()
  var init = [{
    type: 'put',
    key: 'color',
    value: 'purple'
  }, {
    type: 'put',
    key: 'food',
    value: 'sushi'
  }, {
    type: 'put',
    key: 'fight',
    value: 'fire'
  }, {
    type: 'put',
    key: 'colo',
    value: 'trolo'
  }, {
    type: 'put',
    key: 'color',
    value: 'blue'
  }, {
    type: 'put',
    key: 'color',
    value: 'pink'
  }]

  var expectedNodes = {
    '3c38d9aa6ad288c8e27da701e17fe99a5b67c8b12fd0469651c80494d36bc4c1': true,
    'd5f61e1ff2b918d1c2a2c4b1732a3c68bd7e3fd64f35019f2f084896d4546298': true,
    'e64329dadee2fb8a113b4c88cfe973aeaa9b523d4dc8510b84ca23f9d5bfbd90': true,
    'c916d458bfb5f27603c5bd93b00f022266712514a59cde749f19220daffc743f': true,
    '2386bfb0de9cf93902a110f5ab07b917ffc0b9ea599cb7f4f8bb6fd1123c866c': true
  }

  it('should populate trie', function (t) {
    trie.checkpoint()
    trie.batch(init, t.end)
  })

  it('should only fetch nodes in the current trie', function (t) {
    var stream = trie.createScratchReadStream()
    stream.on('data', function (d) {
      var key = d.key.toString('hex')
      t.ok(!!expectedNodes[key])
      delete expectedNodes[key]
    })
    stream.on('end', function () {
      t.equal(Object.keys(expectedNodes).length, 0)
      t.end()
    })
  })
})
