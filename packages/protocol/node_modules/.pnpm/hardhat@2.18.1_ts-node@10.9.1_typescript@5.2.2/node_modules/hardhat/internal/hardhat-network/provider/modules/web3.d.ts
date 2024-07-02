import { HardhatNode } from "../node";
export declare class Web3Module {
    private readonly _node;
    constructor(_node: HardhatNode);
    processRequest(method: string, params?: any[]): Promise<any>;
    private _clientVersionParams;
    private _clientVersionAction;
    private _sha3Params;
    private _sha3Action;
}
//# sourceMappingURL=web3.d.ts.map