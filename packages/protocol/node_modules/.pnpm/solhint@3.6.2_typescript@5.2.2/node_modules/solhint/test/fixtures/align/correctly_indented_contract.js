const { multiLine } = require('../../common/contract-builder')

module.exports = multiLine(
  'contract A {                                       ',
  '    function a() public view returns (uint, uint) {',
  '        return (1, 2);                             ',
  '    }                                              ',
  '                                                   ',
  '    function b() public view returns (uint, uint) {',
  '        (                                          ',
  '            uint c,                                ',
  '            uint d                                 ',
  '        ) = a();                                   ',
  '        return (c, d);                             ',
  '    }                                              ',
  '}                                                  '
)
