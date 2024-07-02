/// <reference types="node" />
import { Common } from "@nomicfoundation/ethereumjs-common";
import { Trie } from "@nomicfoundation/ethereumjs-trie";
import { HardforkName } from "../../../util/hardforks";
import { HardhatBlockchain } from "../HardhatBlockchain";
import { LocalNodeConfig } from "../node-types";
export declare function putGenesisBlock(blockchain: HardhatBlockchain, common: Common, { initialDate, blockGasLimit: initialBlockGasLimit }: LocalNodeConfig, stateTrie: Trie, hardfork: HardforkName, initialMixHash: Buffer, initialBaseFee?: bigint): Promise<void>;
//# sourceMappingURL=putGenesisBlock.d.ts.map