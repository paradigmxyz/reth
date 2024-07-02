#!/usr/bin/env node
import { tsGenerator } from 'ts-generator'

import { parseArgs } from './parseArgs'
import { TypeChain } from '../TypeChain'
import { logger } from '../utils/logger'

async function main() {
  ;(global as any).IS_CLI = true
  const options = parseArgs()
  const cwd = process.cwd()

  await tsGenerator({ cwd, loggingLvl: 'info' }, new TypeChain({ cwd, rawConfig: options }))
}

main().catch((e) => {
  logger.error('Error occured: ', e.message)

  const stackTracesEnabled = process.argv.includes('--show-stack-traces')
  if (stackTracesEnabled) {
    logger.error('Stack trace: ', e.stack)
  } else {
    logger.error('Run with --show-stack-traces to see the full stacktrace')
  }
  process.exit(1)
})
