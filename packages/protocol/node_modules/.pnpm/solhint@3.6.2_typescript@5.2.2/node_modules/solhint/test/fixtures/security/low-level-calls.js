const WARN_LOW_LEVEL_CODES = [
  'anyAddress.call(code);',
  'a.callcode(test1);',
  'a.delegatecall(test1);',
  'anyAddress.call.value(code)();',
]
const ALLOWED_LOW_LEVEL_CODES = ['anyAddress.call{value: 1 ether}("");']

module.exports = [WARN_LOW_LEVEL_CODES, ALLOWED_LOW_LEVEL_CODES]
