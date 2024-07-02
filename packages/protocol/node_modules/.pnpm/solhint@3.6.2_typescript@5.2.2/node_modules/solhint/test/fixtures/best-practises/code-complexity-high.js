const { multiLine } = require('../../common/contract-builder')

module.exports = multiLine(
  ' if (a > b) {                   ',
  '   if (b > c) {                 ',
  '     if (c > d) {               ',
  '       if (d > e) {             ',
  '       } else {                 ',
  '       }                        ',
  '     }                          ',
  '   }                            ',
  ' }                              ',
  'for (i = 0; i < b; i += 1) { }  ',
  'do { d++; } while (b > c);       ',
  'while (d > e) { }               '
)
