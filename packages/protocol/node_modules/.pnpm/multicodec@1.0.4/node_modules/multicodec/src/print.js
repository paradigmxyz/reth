'use strict'

const table = require('./base-table.json')

// map for code -> print friendly name
const tableByCode = {}

for (const [name, code] of Object.entries(table)) {
  if (tableByCode[code] === undefined) tableByCode[code] = name
}

module.exports = Object.freeze(tableByCode)
