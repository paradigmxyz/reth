function isFallbackFunction(node) {
  return isFunctionDefinition(node) && node.isFallback
}

function isReceiveFunction(node) {
  return isFunctionDefinition(node) && node.isReceiveEther
}

function isFunctionDefinition(node) {
  return node.type === 'FunctionDefinition'
}

function isStructDefinition(node) {
  return node.type === 'StructDefinition'
}

function isEnumDefinition(node) {
  return node.type === 'EnumDefinition'
}

module.exports = {
  isFallbackFunction,
  isReceiveFunction,
  isFunctionDefinition,
  isStructDefinition,
  isEnumDefinition,
}
