const async = require('async')
const ethUtil = require('ethereumjs-util')
const ConcatStream = require('concat-stream')
const lookupOpInfo = require('./opCodes')
const createVmTraceStream = require('./index').createVmTraceStream

module.exports = traceTx


function traceTx(provider, txHash, cb) {
  const traceStream = createVmTraceStream(provider, txHash)
  traceStream.on('error', (err) => cb(err) )
  const concatStream = ConcatStream({ encoding: 'object' }, function(vmOutput){
    const result = parseVmOutput(vmOutput)
    cb(null, result)
  })
  traceStream.pipe(concatStream)
}

function parseVmOutput(vmOutput) {
  const txParams = vmOutput.find(datum => datum.type === 'tx').data
  const txResults = vmOutput.find(datum => datum.type === 'results').data
  const traceSteps = vmOutput
    .filter(datum => datum.type === 'step')
    .map(datum => datum.data)
  const structLogs = traceSteps
    .map((step, index) => {
      const nextStep = traceSteps[index+1]
      const nextGasBn = nextStep ? nextStep.gasLeft : txResults.vm.gas
      const gasUsedBn = step.gasLeft.sub(nextGasBn)
      return formatStep(step, gasUsedBn)
    })

  return {
    failed: !txResults.vm.exception,
    gas: txResults.gasUsed.toNumber(),
    returnValue: txResults.vm.return.toString('hex'),
    structLogs,
  }
}

function formatStep(step, gasUsedBn) {
  const opInfo = step.opcode
  const cleanMemory = step.memory.filter(entry => entry !== 'undefined')

  return {
    depth: step.depth + 1,
    error: null,
    gas: step.gasLeft.toNumber(),
    // gasCost: does not include dynamic component
    gasCost: gasUsedBn.toNumber(),
    memory: cleanMemory.length ? formatMemory(cleanMemory) : null,
    op: opInfo.name,
    pc: step.pc,
    stack: formatMemory(step.stack),
    // storage: not available
    storage: {},
  }
}

function formatMemory(memory) {
  return memory.map(entry => ethUtil.setLengthLeft(entry, 32).toString('hex'))
}

// {
//     depth: 1,
//     error: "",
//     gas: 100000,
//     gasCost: 0,
//     memory: ["0000000000000000000000000000000000000000000000000000000000000006", "0000000000000000000000000000000000000000000000000000000000000000", "0000000000000000000000000000000000000000000000000000000000000060"],
//     op: "STOP",
//     pc: 120,
//     stack: ["00000000000000000000000000000000000000000000000000000000d67cbec9"],
//     storage: {
//       0000000000000000000000000000000000000000000000000000000000000004: "8241fa522772837f0d05511f20caa6da1d5a3209000000000000000400000001",
//       0000000000000000000000000000000000000000000000000000000000000006: "0000000000000000000000000000000000000000000000000000000000000001",
//       f652222313e28459528d920b65115c16c04f3efc82aaedc97be59f3f377c0d3f: "00000000000000000000000002e816afc1b5c0f39852131959d946eb3b07b5ad"
//     }
// }]

// { type: 'step',
//   data:
//    { index: 101,
//      opcode: { name: 'CALL', fee: 40, in: 7, out: 1, dynamic: true },
//      stack:
//       [ <Buffer 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 62 5e 84 7d>,
//         <Buffer 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 03 17>,
//         <Buffer 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00>,
//         <Buffer 00 00 00 00 00 00 00 00 00 00 00 00 96 98 37 49 89 44 ae 1d c0 dc ac 2d 0c 65 63 4c 88 72 9b 2d>,
//         <Buffer 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00>,
//         <Buffer 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 06 3b 9a 45 87 d6 07 85 49>,
//         <Buffer 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 60>,
//         <Buffer 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00>,
//         <Buffer 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 60>,
//         <Buffer 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00>,
//         <Buffer 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 60>,
//         <Buffer 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 06 3b 9a 45 87 d6 07 85 49>,
//         <Buffer 00 00 00 00 00 00 00 00 00 00 00 00 96 98 37 49 89 44 ae 1d c0 dc ac 2d 0c 65 63 4c 88 72 9b 2d>,
//         <Buffer 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00> ],
//      memory:
//       [ <64 empty items>,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         0,
//         96 ],
//      address: <Buffer f8 35 a0 24 7b 00 63 c0 4e f2 20 06 eb e5 7c 5f 11 97 7c c4>,
//      pc: 1298,
//      depth: 0 } }

// { type: 'tx',
//   data:
//    { blockHash: '0x67bcfb9255041ef50aeeeadb7ebca5de18e9ca06570202ea835aa51f0aec8e71',
//      blockNumber: '0x1a3cad',
//      from: '0xf35e2cc8e6523d683ed44870f5b7cc785051a77d',
//      gas: '0x47e7c4',
//      gasPrice: '0x60db88400',
//      hash: '0xc0b6d5916bff007ef3a349b9191300e210a5fbb1db7f1cece50184c479947bc3',
//      input: '0x625e847d',
//      nonce: '0xa8',
//      to: '0xf835a0247b0063c04ef22006ebe57c5f11977cc4',
//      transactionIndex: '0x0',
//      value: '0x63b9a44f1fd00e3c0',
//      v: '0x1b',
//      r: '0x288b866f6268e34415f2f9fe39d7a718e716a1b44f355996120b11c0d4a87ce6',
//      s: '0x6f6148d08b6b1a8c4bbd82cdebc31f01c6f8e659b2df516de579a5ea27a922be' } }

// { type: 'results',
//   data:
//    { gasUsed: <BN: 310fc>,
//      createdAddress: undefined,
//      vm:
//       { runState: [Object],
//         suicides: {},
//         suicideTo: undefined,
//         gasRefund: <BN: 15f90>,
//         exception: 1,
//         exceptionError: null,
//         logs: [Array],
//         gas: <BN: ad1b4>,
//         return: <Buffer 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 01>,
//         gasUsed: <BN: 41a34> },
//      bloom:
//       { bitvector: <Buffer 00 00 00 00 00 00 80 00 00 00 00 02 00 00 00 00 80 00 01 00 00 00 00 00 20 00 00 00 00 00 00 00 00 00 00 00 00 00 00 80 00 00 00 08 01 00 00 00 00 00 ... > },
//      amountSpent: <BN: e475e4be8e000> } }
