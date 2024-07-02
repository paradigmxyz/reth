import { ResolverEngine } from "@resolver-engine/core";
import { ImportFile } from "./parsers/importparser";
export declare function findImports(data: ImportFile): string[];
/**
 * Starts with roots and traverses the whole depedency tree of imports, returning an array of sources
 * @param roots
 * @param workingDir What's the starting working dir for resolving relative imports in roots
 * @param resolver
 */
export declare function gatherSources(roots: string[], workingDir: string, resolver: ResolverEngine<ImportFile>): Promise<ImportFile[]>;
/**
 * This function gathers sources and **REWRITES IMPORTS** inside the source files into resolved, absolute paths instead of using shortcut forms
 * Because the remapping api in solc is not compatible with multiple existing projects and frameworks, changing relative paths to absolute paths
 * makes us avoid any need for finding imports after starting the solc compilation
 * @param roots
 * @param workingDir What's the starting working dir for resolving relative imports in roots
 * @param resolver
 */
export declare function gatherSourcesAndCanonizeImports(roots: string[], workingDir: string, resolver: ResolverEngine<ImportFile>): Promise<ImportFile[]>;
