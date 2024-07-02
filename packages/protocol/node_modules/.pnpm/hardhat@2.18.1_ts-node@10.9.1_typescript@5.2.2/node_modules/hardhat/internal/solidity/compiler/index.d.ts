import { CompilerInput, CompilerOutput } from "../../../types";
export interface ICompiler {
    compile(input: CompilerInput): Promise<CompilerOutput>;
}
export declare class Compiler implements ICompiler {
    private _pathToSolcJs;
    private _loadedSolc?;
    constructor(_pathToSolcJs: string);
    compile(input: CompilerInput): Promise<any>;
    getSolc(): Promise<any>;
    /**
     * This function loads the compiler sources bypassing any require hook.
     *
     * The compiler is a huge asm.js file, and using a simple require may trigger
     * babel/register and hang the process.
     */
    private _loadCompilerSources;
}
export declare class NativeCompiler implements ICompiler {
    private _pathToSolc;
    constructor(_pathToSolc: string);
    compile(input: CompilerInput): Promise<any>;
}
//# sourceMappingURL=index.d.ts.map