import type { BigIntLike } from "@nomicfoundation/ethereumjs-util";
import { AccessListEIP2930TxData, TxData } from "@nomicfoundation/ethereumjs-tx";
import { RpcTransaction } from "../../../core/jsonrpc/types/output/transaction";
interface FeeMarketEIP1559TxData extends AccessListEIP2930TxData {
    maxPriorityFeePerGas?: BigIntLike;
    maxFeePerGas?: BigIntLike;
}
export declare function rpcToTxData(rpcTransaction: RpcTransaction): TxData | AccessListEIP2930TxData | FeeMarketEIP1559TxData;
export {};
//# sourceMappingURL=rpcToTxData.d.ts.map