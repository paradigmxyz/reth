declare class ModuleError extends Error {
  /**
   * @param message Error message
   */
  constructor(message: string, options?: {
    code?: string
    cause?: Error
    expected?: boolean
    transient?: boolean
  } | undefined)

  code: string | undefined
  expected: boolean | undefined
  transient: boolean | undefined
  cause: Error | undefined
}

export = ModuleError
