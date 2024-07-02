'use strict'
const commandLineArgs = require('../')
const commandLineUsage = require('command-line-usage')

/*
  This example shows typical use alongside command-line-usage
  https://github.com/75lb/command-line-usage
*/

const optionDefinitions = [
  {
    name: 'help',
    alias: 'h',
    type: Boolean,
    description: 'Display this usage guide.'
  },
  {
    name: 'src',
    type: String,
    multiple: true,
    defaultOption: true,
    description: 'The input files to process',
    typeLabel: '<files>' },
  {
    name: 'timeout',
    alias: 't',
    type: Number,
    description: 'Timeout value in ms',
    typeLabel: '<ms>' },
  {
    name: 'log',
    alias: 'l',
    type: Boolean,
    description: 'info, warn or error'
  }
]

const options = commandLineArgs(optionDefinitions)

if (options.help) {
  const usage = commandLineUsage([
    {
      header: 'Typical Example',
      content: 'A simple example demonstrating typical usage.'
    },
    {
      header: 'Options',
      optionList: optionDefinitions
    },
    {
      content: 'Project home: [underline]{https://github.com/me/example}'
    }
  ])
  console.log(usage)
}

/*

Example output:

$ node example/typical.js --help

Typical Example

  A simple example demonstrating typical usage.

Options

  -h, --help           Display this usage guide.
  --src <files>        The input files to process
  -t, --timeout <ms>   Timeout value in ms
  -l, --log            info, warn or error

  Project home: https://github.com/me/example

*/
