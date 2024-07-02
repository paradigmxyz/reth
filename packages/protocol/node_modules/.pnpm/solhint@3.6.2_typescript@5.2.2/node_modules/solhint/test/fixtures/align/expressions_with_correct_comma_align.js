const { funcWith, contractWith } = require('../../common/contract-builder')

module.exports = [
  funcWith('var (a, b,) = test1.test2(); a + b;'),
  funcWith('test(1, 2, b);'),
  contractWith('function b(uint a, uintc) public {}'),
  contractWith('enum A {Test1, Test2}'),
  funcWith('var (a, , , b) = test1.test2(); a + b;'),
]
