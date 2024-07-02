export function indent(text: string, amount: number, amount0 = amount): string {
  return text.replace(/^/gm, (_, i) => ' '.repeat(i === 0 ? amount0 : amount));
}
