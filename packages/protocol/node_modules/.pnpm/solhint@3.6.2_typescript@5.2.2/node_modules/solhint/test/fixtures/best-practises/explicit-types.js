const VAR_DECLARATIONS = {
  uint256: {
    code: 'uint256 public varUint256;',
    errorsImplicit: 1,
    errorsExplicit: 0,
  },

  uint: {
    code: 'uint public varUint;',
    errorsImplicit: 0,
    errorsExplicit: 1,
  },

  int256: {
    code: 'int256 public varInt256;',
    errorsImplicit: 1,
    errorsExplicit: 0,
  },

  int: {
    code: 'int public varInt;',
    errorsImplicit: 0,
    errorsExplicit: 1,
  },

  ufixed128x18: {
    code: 'ufixed128x18 public varUfixed128x18;',
    errorsImplicit: 1,
    errorsExplicit: 0,
  },

  ufixed: {
    code: 'ufixed public varUfixed;',
    errorsImplicit: 0,
    errorsExplicit: 1,
  },

  fixed128x18: {
    code: 'fixed128x18 public varFixed128x18;',
    errorsImplicit: 1,
    errorsExplicit: 0,
  },

  fixed: {
    code: 'fixed public varFixed;',
    errorsImplicit: 0,
    errorsExplicit: 1,
  },

  functionParameterAndReturns1: {
    code: 'function withUint256(uint256 varUint256, uint varUint) public returns(int256 returnInt256) {}',
    errorsImplicit: 2,
    errorsExplicit: 1,
  },

  functionParameterAndReturns2: {
    code: 'function withUint256(uint varUint, uint varUint) public returns(int returnInt) {}',
    errorsImplicit: 0,
    errorsExplicit: 3,
  },

  eventParameter: {
    code: 'event EventWithInt256(int256 varWithInt256, address addressVar, bool boolVat, int varWithInt);',
    errorsImplicit: 1,
    errorsExplicit: 1,
  },

  regularMap1: {
    code: 'mapping(uint256 => fixed128x18) public mapWithFixed128x18;',
    errorsImplicit: 2,
    errorsExplicit: 0,
  },

  regularMap2: {
    code: 'mapping(uint => fixed128x18) public mapWithFixed128x18;',
    errorsImplicit: 1,
    errorsExplicit: 1,
  },

  mapOfMapping: {
    code: 'mapping(uint => mapping(fixed128x18 => int256)) mapOfMap;',
    errorsImplicit: 2,
    errorsExplicit: 1,
  },

  struct: {
    code: 'struct Estructura { ufixed128x18 varWithUfixed128x18; uint varUint; }',
    errorsImplicit: 1,
    errorsExplicit: 1,
  },

  mapOfStruct: {
    code: 'struct Estructura { ufixed128x18 varWithUfixed128x18; uint varUint; mapping(uint256 => Estructura) mapOfStruct; }',
    errorsImplicit: 2,
    errorsExplicit: 1,
  },

  structWithMap: {
    code: 'struct Estructura { ufixed varWithUfixed; uint varUint; mapping(address => uint256) structWithMap; } ',
    errorsImplicit: 1,
    errorsExplicit: 2,
  },

  mapOfArrayStructWithMap: {
    code: 'struct Estructura { ufixed varWithUfixed; uint varUint; mapping(address => uint256) structWithMap; } mapping(uint256 => Estructura[]) mapOfArrayStructWithMap;',
    errorsImplicit: 2,
    errorsExplicit: 2,
  },

  regularArray: {
    code: 'uint256[] public arr;',
    errorsImplicit: 1,
    errorsExplicit: 0,
  },

  fixedArray: {
    code: 'uint[] public arr = [1,2,3];',
    errorsImplicit: 0,
    errorsExplicit: 1,
  },

  fixedArrayOfArrays: {
    code: 'uint256[] public arr = [[1,2,3]];',
    errorsImplicit: 1,
    errorsExplicit: 0,
  },

  mapOfFixedArray: {
    code: 'uint[] public arr = [1,2,3]; mapping(uint256 => arr) mapOfFixedArray;',
    errorsImplicit: 1,
    errorsExplicit: 1,
  },
}

module.exports = VAR_DECLARATIONS
