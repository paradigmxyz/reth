'use strict'

const bent = require('bent')
const path = require('path')

const get = bent('string')
const url = 'https://raw.githubusercontent.com/multiformats/multicodec/master/table.csv'
const fs = require('fs')

const parse = async function * () {
  const str = await get(url)
  const lines = str.split('\n')
  lines.shift()
  for (const line of lines) {
    if (!line.length) continue
    const [name, tag, code] = line.split(',')
    yield { name: name.trim(), tag: tag.trim(), code: parseInt(code.trim(), 16) }
  }
}

const run = async () => {
  const table = {}

  for await (const { name, code } of parse()) {
    table[name] = code
  }

  fs.writeFileSync(path.join(__dirname, '../src/base-table.json'), JSON.stringify(table, null, 2))
}
run()
