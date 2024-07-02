const async = require('async')
const { bufferToHex, bufferToInt, setLengthLeft } = require('ethereumjs-util')
const endOfStream = require('end-of-stream')
const through = require('through2').obj

module.exports = createCallTraceTransform


function createCallTraceTransform() {

  const callTrace = { accounts: {}, calls: [], stackFrames: [], logs: [] }

  const callTraceTransform = through((vmTraceDatum, enc, cb) => {
    let result
    try {
      result = handleDatum(vmTraceDatum)
    } catch (err) {
      return cb(err)
    }
    if (result) callTraceTransform.push(result)
    return cb()
  })

  return callTraceTransform

  function handleDatum(vmTraceDatum){
    switch (vmTraceDatum.type) {
      case 'tx':
        return initalMessage(vmTraceDatum.data)
      case 'step':
        return analyzeStep(vmTraceDatum.data)
    }
  }

  function initalMessage(txParams) {
    const message = messageFromTx(txParams)
    recordMessage(message)
    const stack = [0]
    message.stack = stack
    recordStack(stack)
    // prepare datum
    const callTraceDatum = {
      type: 'message',
      data: message,
    }
    return callTraceDatum
  }

  function analyzeStep(step){
    switch(step.opcode.name) {
      case 'CALL':
      case 'CALLCODE':
        return analyzeCall(step, true)

      case 'DELEGATECALL':
      case 'STATICCALL':
        return analyzeCall(step, false)

      case 'LOG0':
      case 'LOG1':
      case 'LOG2':
      case 'LOG3':
      case 'LOG4':
        return analyzeLog(step)

      default:
        // return console.log(`${step.index} ${step.opcode.name}`)
    }
  }

  function analyzeCall(step, includeValue) {
    const message = messageFromStep(step, includeValue)
    recordMessage(message)
    const prevStack = callTrace.stackFrames.slice(-1)[0]
    const stack = stackFromMessage(prevStack, message)
    message.stack = stack
    recordStack(stack)
    // update calls and call stack
    callTrace.stackFrames.push(stack)
    callTrace.calls.push(message)
    // prepare datum
    const callTraceDatum = {
      type: 'message',
      data: message,
    }
    return callTraceDatum
  }

  function analyzeLog(step) {
    const numTopics = step.opcode.in - 2
    const memOffset = bufferToInt(step.stack.pop())
    const memLength = bufferToInt(step.stack.pop())
    const topics = step.stack.slice(0, numTopics).map(function(topic){
      return bufferToHex(setLengthLeft(topic, 32))
    })
    const log = {
      callIndex: callTrace.calls.length - 1,
      address: bufferToHex(step.address),
      topics: topics,
      // TODO: load data from memory
      // data: null,
    }
    const callTraceDatum = {
      type: 'log',
      data: log,
    }
    return callTraceDatum
  }

  function messageFromTx(txParams){
    const message = {
      sequence:    0,
      depth:       0,
      fromAddress: bufferToHex(txParams.from),
      gasLimit:    bufferToHex(txParams.gas || txParams.gasLimit),
      toAddress:   bufferToHex(txParams.to),
      value:       bufferToHex(txParams.value),
      data:        bufferToHex(txParams.data || txParams.input),
    }
    return message
  }

  function messageFromStep(step, includeValue){
    const depth = step.depth + 1
    // from the stack (order is important)
    const gasLimit  = bufferToHex(step.stack.pop())
    const toAddress = bufferToHex(setLengthLeft(step.stack.pop(), 20))
    const value     = includeValue ? bufferToHex(step.stack.pop()) : '0x0'
    const inOffset  = bufferToInt(step.stack.pop())
    const inLength  = bufferToInt(step.stack.pop())
    // const outOffset = bufferToInt(step.stack.pop())
    // const outLength = bufferToInt(step.stack.pop())

    const data = bufferToHex(memLoad(step.memory, inOffset, inLength))

    const callParams = {
      opcode:      step.opcode.name,
      sequence:    callTrace.calls.length,
      depth:       depth,
      fromAddress: bufferToHex(step.address),
      gasLimit:    gasLimit,
      toAddress:   toAddress,
      value:       value,
      data:        data,
    }
    return callParams
  }

  function stackFromMessage(prevStack, msgParams){
    const topOfStackCallIndex = prevStack.slice(-1)[0]
    const topOfStack = callTrace.calls[topOfStackCallIndex]
    const newStack = prevStack.slice()
    const prevDepth = topOfStack.depth
    const itemsToRemove = 1 + prevDepth - msgParams.depth
    // remove old calls
    newStack.splice(newStack.length-itemsToRemove)
    // add new call
    const messageIndex = callTrace.calls.indexOf(msgParams)
    newStack.push(messageIndex)
    return newStack
  }

  function recordStack(stack){
    callTrace.stackFrames.push(stack)
  }

  function recordMessage(message){
    callTrace.calls.push(message)
  }

}


// from ethereumjs-vm
function memLoad(memory, offset, length) {
  const loaded = memory.slice(offset, offset + length)
  // fill the remaining lenth with zeros
  for (let i = loaded.length; i < length; i++) {
    loaded.push(0)
  }
  return new Buffer(loaded)
}
