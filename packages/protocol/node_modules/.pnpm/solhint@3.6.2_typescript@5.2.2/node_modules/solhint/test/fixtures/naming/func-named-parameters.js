const FUNCTION_CALLS_ERRORS = {
  err1: {
    code: 'funcName(_sender, amount, receiver, token1, token2, token3);',
    minUnnamed: 5,
  },

  err2: {
    code: 'funcName(_sender, amount, receiver, token1, token2);',
    minUnnamed: 4,
  },

  err3: {
    code: 'funcName(_sender, amount, receiver, token1, token2);',
    minUnnamed: 0,
  },
}

const FUNCTION_CALLS_OK = {
  ok1: {
    code: 'funcName();',
    minUnnamed: 0,
  },

  ok2: {
    code: 'address(0);',
    minUnnamed: 10,
  },

  ok3: {
    code: 'funcName({ sender: _sender, amount: _amount, receiver: _receiver });',
    minUnnamed: 1,
  },

  ok4: {
    code: 'assert(1 == 3);',
    minUnnamed: 1,
  },

  ok5: {
    code: 'bytes foo = abi.encodeWithSelector(hex"0102030405060708", uint16(0xff00));',
    minUnnamed: 2,
  },

  ok6: {
    code: 'funcName({ sender: _sender, amount: _amount, receiver: _receiver, token1: _token1, token2: _token2 });',
    minUnnamed: 5,
  },

  ok7: {
    code: 'new BeaconProxy(address(0),abi.encodeWithSelector(bytes4(""),address(0),address(0),address(0)));',
    minUnnamed: 2,
  },

  ok8: {
    code: 'salt = keccak256(abi.encode(msg.sender, block.chainid, salt));',
    minUnnamed: 0,
  },

  ok9: {
    code: 'require(foobar != address(0), "foobar must a valid address");',
    minUnnamed: 0,
  },
}

module.exports = { FUNCTION_CALLS_ERRORS, FUNCTION_CALLS_OK }
