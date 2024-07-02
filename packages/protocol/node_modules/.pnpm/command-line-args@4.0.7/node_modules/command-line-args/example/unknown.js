'use strict'
const commandLineArgs = require('../')

const optionDefinitions = [
  { name: 'depth', type: Number },
  { name: 'files', alias: 'f', type: String, multiple: true, defaultOption: true }
]

const options = commandLineArgs(optionDefinitions, { partial: true })

console.log(options)

/*

Example output:

$ node example/unknown.js --depth 3 package.json README.md --unknown1 --unknown2
{ depth: 3,
  files: [ 'package.json', 'README.md' ],
  _unknown: [ '--unknown1', '--unknown2' ] }

*/
