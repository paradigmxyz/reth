// itemize returns a markdown-like list of the items

// itemize['a\nb', 'c'] =
// - a
//   b
// - c

import { indent } from './indent';

export function itemize(...items: string[]): string {
  return itemizeWith('-', ...items);
}

export function itemizeWith(bullet: string, ...items: string[]): string {
  return items.map(item => bullet + indent(item, 2, 1)).join('\n');
}
