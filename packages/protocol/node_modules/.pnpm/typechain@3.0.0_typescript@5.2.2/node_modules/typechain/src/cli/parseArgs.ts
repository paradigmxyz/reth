import commandLineArgs from 'command-line-args'

const DEFAULT_GLOB_PATTERN = '**/*.abi'

export interface IOptions {
  files: string
  target: string
  outDir?: string
}

export function parseArgs(): IOptions {
  const optionDefinitions = [
    { name: 'glob', type: String, defaultOption: true },
    { name: 'target', type: String },
    { name: 'outDir', type: String },
    { name: 'show-stack-traces', type: Boolean },
  ]

  const rawOptions = commandLineArgs(optionDefinitions)

  return {
    files: rawOptions.glob || DEFAULT_GLOB_PATTERN,
    outDir: rawOptions.outDir,
    target: rawOptions.target,
  }
}
