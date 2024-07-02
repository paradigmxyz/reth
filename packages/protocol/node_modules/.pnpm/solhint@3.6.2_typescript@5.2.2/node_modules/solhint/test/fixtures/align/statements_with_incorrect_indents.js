const { multiLine } = require('../../common/contract-builder')

module.exports = [
  'if(a > b) {}',
  'if (a > b ) {} else {}',
  'while ( a > b) {}',
  'do {} while (a > b );',
  'for (;; ) {}',
  'for (uint i = 0;; ) {}',
  'for (;a < b; ) {}',
  'for (;;i += 1) {}',
  'for (uint i = 0;;i += 1) {}',
  'for (uint i = 0;i += 1;) {}',
  'for (;a < b; i += 1) {}',
  'for (uint i = 0;a < b; i += 1) {}',
  multiLine(
    'if (a < b) { ',
    '  test1();   ',
    '}            ',
    'else {       ',
    '  test2();   ',
    '}            '
  ),
  multiLine('do {           ', '  test1();     ', '}              ', 'while (a < b); '),
]
