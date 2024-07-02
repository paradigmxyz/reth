/// <reference types="node" />
import { OptionalRpcNewBlockTag, RpcNewBlockTag } from "../../../core/jsonrpc/types/input/blockTag";
import { HardhatNode } from "../node";
import { RpcCallRequest } from "../../../core/jsonrpc/types/input/callRequest";
import { CallParams } from "../node-types";
import { RpcAccessList } from "../../../core/jsonrpc/types/access-list";
export declare class Base {
    protected readonly _node: HardhatNode;
    constructor(_node: HardhatNode);
    resolveNewBlockTag(newBlockTag: OptionalRpcNewBlockTag, defaultValue?: RpcNewBlockTag): Promise<bigint | "pending">;
    rpcCallRequestToNodeCallParams(rpcCall: RpcCallRequest): Promise<CallParams>;
    protected _rpcAccessListToNodeAccessList(rpcAccessList: RpcAccessList): Array<[Buffer, Buffer[]]>;
    protected _checkPostMergeBlockTags(blockTag: "safe" | "finalized"): void;
    protected _newBlockTagToString(tag: RpcNewBlockTag): string;
    private _getDefaultCallFrom;
}
//# sourceMappingURL=base.d.ts.map