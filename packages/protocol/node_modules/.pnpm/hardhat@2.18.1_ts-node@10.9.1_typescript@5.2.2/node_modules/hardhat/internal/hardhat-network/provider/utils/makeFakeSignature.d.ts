import { AccessListEIP2930Transaction, FeeMarketEIP1559Transaction, TxData } from "@nomicfoundation/ethereumjs-tx";
import { Address } from "@nomicfoundation/ethereumjs-util";
export declare function makeFakeSignature(tx: TxData | AccessListEIP2930Transaction | FeeMarketEIP1559Transaction, sender: Address): {
    r: number;
    s: number;
};
//# sourceMappingURL=makeFakeSignature.d.ts.map