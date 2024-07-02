import { TsGeneratorPlugin, TFileDesc, TContext, TOutput } from 'ts-generator'
import _ = require('lodash')
import { compact } from 'lodash'
import { debug } from './utils/debug'
import { isAbsolute, join } from 'path'

export interface ITypeChainCfg {
  target: string
  outDir?: string
}

/**
 * Proxies calls to real implementation that is selected based on target parameter.
 */
export class TypeChain extends TsGeneratorPlugin {
  name = 'TypeChain'
  private realImpl: TsGeneratorPlugin

  constructor(ctx: TContext<ITypeChainCfg>) {
    super(ctx)

    this.realImpl = this.findRealImpl(ctx)
  }

  private findRealImpl(ctx: TContext<ITypeChainCfg>): TsGeneratorPlugin {
    const target = ctx.rawConfig.target
    if (!target) {
      throw new Error(`Please provide --target parameter!`)
    }

    const possiblePaths = [
      process.env.NODE_ENV === 'test' && `../../typechain-target-${target}/lib/index`, // only for tests
      `typechain-target-${target}`, //external module
      `@typechain/${target}`, //external module
      ensureAbsPath(target), // path
    ]

    const moduleInfo = _(possiblePaths).compact().map(tryRequire).compact().first()

    if (!moduleInfo || !moduleInfo.module.default) {
      throw new Error(
        `Couldn't find ${ctx.rawConfig.target}. Tried loading: ${compact(possiblePaths).join(
          ', ',
        )}.\nPerhaps you forgot to install typechain-target-${target}?`,
      )
    }

    debug('Plugin found at', moduleInfo.path)

    return new moduleInfo.module.default(ctx)
  }

  beforeRun(): TOutput | Promise<TOutput> {
    return this.realImpl.beforeRun()
  }

  transformFile(file: TFileDesc): TOutput | Promise<TOutput> {
    return this.realImpl.transformFile(file)
  }

  afterRun(): TOutput | Promise<TOutput> {
    return this.realImpl.afterRun()
  }
}

function tryRequire(name: string): { module: any; name: string; path: string } | undefined {
  try {
    const module = {
      module: require(name),
      name,
      path: require.resolve(name),
    }
    debug('Load successfully: ', name)
    return module
  } catch (e) {
    debug("Couldn't load: ", name)
  }
}

function ensureAbsPath(path: string): string {
  if (isAbsolute(path)) {
    return path
  }
  return join(process.cwd(), path)
}
