const { multiLine } = require('../../common/contract-builder')

module.exports = multiLine(
  '    contract A {        ',
  '        uint private a; ',
  '    }                   '
)
