export class Logger {
  log(...args: any[]) {
    if (!(global as any).IS_CLI) {
      return
    }

    console.log(...args)
  }
  warn(...args: any[]) {
    if (!(global as any).IS_CLI) {
      return
    }

    console.warn(...args)
  }

  error(...args: any[]) {
    if (!(global as any).IS_CLI) {
      return
    }

    console.error(...args)
  }
}

export const logger = new Logger()
