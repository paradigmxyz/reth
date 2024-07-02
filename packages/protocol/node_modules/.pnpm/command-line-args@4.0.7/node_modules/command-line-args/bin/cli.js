#!/usr/bin/env node
'use strict'
const commandLineArgs = require('../')
const os = require('os')
const fs = require('fs')
const path = require('path')

const tmpPath = path.join(os.tmpdir(), Date.now() + '-cla.js')

process.stdin
  .pipe(fs.createWriteStream(tmpPath))
  .on('close', parseCla)

function parseCla () {
  const cliOptions = require(tmpPath)
  fs.unlinkSync(tmpPath)

  try {
    console.log(commandLineArgs(cliOptions))
  } catch (err) {
    console.error(err.message)
    process.exitCode = 1
  }
}
