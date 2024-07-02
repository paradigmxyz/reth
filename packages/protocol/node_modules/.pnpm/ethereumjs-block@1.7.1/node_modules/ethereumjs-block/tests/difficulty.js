// const testing = require('ethereumjs-testing')
const utils = require('ethereumjs-util')
const tape = require('tape')
const Block = require('../')
const BN = utils.BN

function normalize (data) {
  Object.keys(data).map(function (i) {
    if (i !== 'homestead' && typeof (data[i]) === 'string') {
      data[i] = utils.isHexPrefixed(data[i]) ? new BN(utils.toBuffer(data[i])) : new BN(data[i])
    }
  })
}

tape('[Header]: difficulty tests', t => {
  function runDifficultyTests (test) {
    normalize(test)

    var parentBlock = new Block()
    parentBlock.header.timestamp = test.parentTimestamp
    parentBlock.header.difficulty = test.parentDifficulty
    parentBlock.header.uncleHash = test.parentUncles

    var block = new Block()
    block.header.timestamp = test.currentTimestamp
    block.header.difficulty = test.currentDifficulty
    block.header.number = test.currentBlockNumber

    var dif = block.header.canonicalDifficulty(parentBlock)
    t.equal(dif.toString(), test.currentDifficulty.toString(), 'test canonicalDifficulty')
    t.assert(block.header.validateDifficulty(parentBlock), 'test validateDifficulty')
  }

  const testData = require('./testdata-difficulty.json')
  for (let testName in testData) {
    runDifficultyTests(testData[testName])
  }
  t.end()

  // Temporarily run local test selection
  // also: implicit testing through ethereumjs-vm tests
  // (no Byzantium difficulty tests available yet)
  /*
  let args = {}
  args.file = /^difficultyHomestead/
  testing.getTestsFromArgs('BasicTests', (fileName, testName, test) => {
    return new Promise((resolve, reject) => {
      runDifficultyTests(test)
      resolve()
    }).catch(err => console.log(err))
  }, args).then(() => {
    t.end()
  })
  */
})
