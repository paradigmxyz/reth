import chalk from 'chalk';

import { indent } from './indent';

let silenced = false;

function log(prefix: string, title: string, lines: string[]): void {
  if (silenced) {
    return;
  }

  const parts = [chalk.yellow.bold(prefix + ':') + ' ' + title + '\n'];

  if (lines.length > 0) {
    parts.push(lines.map(l => indent(l, 4) + '\n').join(''));
  }

  console.error(parts.join('\n'));
}

export function logNote(title: string, lines: string[] = []): void {
  log('Note', title, lines);
}

export function logWarning(title: string, lines: string[] = []): void {
  log('Warning', title, lines);
}

export function silenceWarnings(): void {
  logWarning(`All subsequent Upgrades warnings will be silenced.`, [
    `Make sure you have manually checked all uses of unsafe flags.`,
  ]);
  silenced = true;
}
