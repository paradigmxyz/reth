import { HardhatNode } from "../node";
import { Base } from "./base";
export declare class DebugModule extends Base {
    constructor(_node: HardhatNode);
    processRequest(method: string, params?: any[]): Promise<any>;
    private _traceCallParams;
    private _traceCallAction;
    private _traceTransactionParams;
    private _traceTransactionAction;
    private _validateTracerParam;
}
//# sourceMappingURL=debug.d.ts.map