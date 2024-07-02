#!/usr/bin/env node

const program = require('commander')
const _ = require('lodash')
const fs = require('fs')
const process = require('process')

const linter = require('./lib/index')
const { loadConfig } = require('./lib/config/config-file')
const { validate } = require('./lib/config/config-validator')
const applyFixes = require('./lib/apply-fixes')
const ruleFixer = require('./lib/rule-fixer')
const packageJson = require('./package.json')

function init() {
  const version = packageJson.version
  program.version(version)

  program
    .name('solhint')
    .usage('[options] <file> [...other_files]')
    .option(
      '-f, --formatter [name]',
      'report formatter name (stylish, table, tap, unix, json, compact)'
    )
    .option('-w, --max-warnings [maxWarningsNumber]', 'number of allowed warnings')
    .option('-c, --config [file_name]', 'file to use as your .solhint.json')
    .option('-q, --quiet', 'report errors only - default: false')
    .option('--ignore-path [file_name]', 'file to use as your .solhintignore')
    .option('--fix', 'automatically fix problems')
    .option('--init', 'create configuration file for solhint')
    .description('Linter for Solidity programming language')
    .action(execMainAction)

  program
    .command('stdin', null, { noHelp: false })
    .option('--filename [file_name]', 'name of file received using STDIN')
    .description(
      'linting of source code data provided to STDIN\nEx: echo "contract Foo { function f1()external view returns(uint256) {} }" | solhint stdin\nEx: solhint stdin --filename "./pathToFilename" -f table'
    )
    .action(processStdin)

  program
    .command('init-config', null, { noHelp: true })
    .description('create configuration file for solhint')
    .action(writeSampleConfigFile)

  program
    .command('list-rules', null, { noHelp: false })
    .description('display covered rules of current .solhint.json')
    .action(listRules)

  if (process.argv.length <= 2) {
    program.help()
  }
  program.parse(process.argv)
}

function execMainAction() {
  if (program.opts().init) {
    writeSampleConfigFile()
  }

  let formatterFn
  try {
    // to check if is a valid formatter before execute linter
    formatterFn = getFormatter(program.opts().formatter)
  } catch (ex) {
    console.error(ex.message)
    process.exit(1)
  }

  const reportLists = program.args.filter(_.isString).map(processPath)
  const reports = _.flatten(reportLists)

  if (program.opts().fix) {
    for (const report of reports) {
      const inputSrc = fs.readFileSync(report.filePath).toString()

      const fixes = _(report.reports)
        .filter((x) => x.fix)
        .map((x) => x.fix(ruleFixer))
        .sort((a, b) => a.range[0] - b.range[0])
        .value()

      const { fixed, output } = applyFixes(fixes, inputSrc)
      if (fixed) {
        report.reports = report.reports.filter((x) => !x.fix)
        fs.writeFileSync(report.filePath, output)
      }
    }
  }

  if (program.opts().quiet) {
    // filter the list of reports, to set errors only.
    reports.forEach((reporter) => {
      reporter.reports = reporter.reports.filter((i) => i.severity === 2)
    })
  }

  printReports(reports, formatterFn)

  exitWithCode(reports)
}

function processStdin(options) {
  const STDIN_FILE = options.filename || 0
  const stdinBuffer = fs.readFileSync(STDIN_FILE)
  const report = processStr(stdinBuffer.toString())
  report.file = options.filename || 'stdin'

  let formatterFn
  try {
    // to check if is a valid formatter before execute linter
    formatterFn = getFormatter(program.opts().formatter)
  } catch (ex) {
    console.error(ex.message)
    process.exit(1)
  }

  const reports = [report]

  printReports(reports, formatterFn)

  exitWithCode(reports)
}

function writeSampleConfigFile() {
  const configPath = '.solhint.json'
  const sampleConfig = `{
  "extends": "solhint:default"
}
`

  if (!fs.existsSync(configPath)) {
    fs.writeFileSync(configPath, sampleConfig)

    console.log('Configuration file created!')
  } else {
    console.log('Configuration file already exists')
  }
}

const readIgnore = _.memoize(() => {
  let ignoreFile = '.solhintignore'
  try {
    if (program.opts().ignorePath) {
      ignoreFile = program.opts().ignorePath
    }

    return fs
      .readFileSync(ignoreFile)
      .toString()
      .split('\n')
      .map((i) => i.trim())
  } catch (e) {
    if (program.opts().ignorePath && e.code === 'ENOENT') {
      console.error(`\nERROR: ${ignoreFile} is not a valid path.`)
    }
    return []
  }
})

const readConfig = _.memoize(() => {
  let config = {}
  try {
    config = loadConfig(program.opts().config)
  } catch (e) {
    console.log(e.message)
    process.exit(1)
  }

  const configExcludeFiles = _.flatten(config.excludedFiles)
  config.excludedFiles = _.concat(configExcludeFiles, readIgnore())

  // validate the configuration before continuing
  validate(config)

  return config
})

function processStr(input) {
  return linter.processStr(input, readConfig())
}

function processPath(path) {
  return linter.processPath(path, readConfig())
}

function areWarningsExceeded(reports) {
  const warningsCount = reports.reduce((acc, i) => acc + i.warningCount, 0)
  const warningsNumberExceeded =
    program.opts().maxWarnings >= 0 && warningsCount > program.opts().maxWarnings

  return { warningsNumberExceeded, warningsCount }
}

function printReports(reports, formatter) {
  const warnings = areWarningsExceeded(reports)
  let finalMessage = ''
  let exitWithOne = false
  if (
    program.opts().maxWarnings &&
    reports &&
    reports.length > 0 &&
    warnings.warningsNumberExceeded
  ) {
    if (!reports[0].errorCount) {
      finalMessage = `Solhint found more warnings than the maximum specified (maximum: ${
        program.opts().maxWarnings
      }, found: ${warnings.warningsCount})`
      exitWithOne = true
    } else {
      finalMessage =
        'Error/s found on rules! [max-warnings] param is ignored. Fixing errors enables max-warnings'
    }
  }

  console.log(formatter(reports), finalMessage || '')

  if (exitWithOne) process.exit(1)
  return reports
}

function getFormatter(formatter) {
  const formatterName = formatter || 'stylish'
  try {
    return require(`./lib/formatters/${formatterName}`)
  } catch (ex) {
    ex.message = `\nThere was a problem loading formatter option: ${
      program.opts().formatter
    } \nError: ${ex.message}`
    throw ex
  }
}

function listRules() {
  if (process.argv.length !== 3) {
    console.log('Error!! no additional parameters after list-rules command')
    process.exit(1)
  }

  const configPath = '.solhint.json'
  if (!fs.existsSync(configPath)) {
    console.log('Error!! Configuration does not exists')
    process.exit(1)
  } else {
    const config = readConfig()
    console.log('\nConfiguration File: \n', config)

    const reportLists = linter.processPath(configPath, config)
    const rulesObject = reportLists[0].config

    console.log('\nRules: \n')
    const orderedRules = Object.keys(rulesObject)
      .sort()
      .reduce((obj, key) => {
        obj[key] = rulesObject[key]
        return obj
      }, {})

    // eslint-disable-next-line func-names
    Object.keys(orderedRules).forEach(function (key) {
      console.log('- ', key, ': ', orderedRules[key])
    })
  }
}

function exitWithCode(reports) {
  const errorsCount = reports.reduce((acc, i) => acc + i.errorCount, 0)
  process.exit(errorsCount > 0 ? 1 : 0)
}

init()
