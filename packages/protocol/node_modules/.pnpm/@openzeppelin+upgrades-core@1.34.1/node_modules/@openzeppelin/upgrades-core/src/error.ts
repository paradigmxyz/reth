import util from 'util';
import chalk from 'chalk';

interface ErrorDescriptor<E> {
  msg: (e: E) => string;
  hint?: (e: E) => string | undefined;
  link?: string;
}

export type ErrorDescriptions<E extends { kind: string }> = {
  [K in E['kind']]: ErrorDescriptor<E & { kind: K }>;
};

function noDetails() {
  return '';
}

export class UpgradesError extends Error {
  constructor(message: string, details = noDetails) {
    super(message + '\n\n' + details());
  }

  [util.inspect.custom](): string {
    return chalk.red.bold('Error:') + ' ' + this.message;
  }
}
