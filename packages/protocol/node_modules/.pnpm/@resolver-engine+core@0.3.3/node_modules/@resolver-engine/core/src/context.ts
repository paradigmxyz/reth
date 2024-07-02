/**
 * Execution context common to parsers and resolvers, alike.
 * Used as a means of passing information, and building upon it, down the chain of execution.
 * Planned future use:
 * - storing already visited paths
 * - system-specific execution info
 */
export interface Context {
  resolver: string;
  cwd?: string;
}
