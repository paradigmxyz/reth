import { SenderTransactions, SerializedTransaction } from "../PoolState";
/**
 * Move as many transactions as possible from the queued list
 * to the pending list.
 *
 * Returns the new lists and the new executable nonce of the sender.
 */
export declare function reorganizeTransactionsLists(pending: SenderTransactions, queued: SenderTransactions, retrieveNonce: (serializedTx: SerializedTransaction) => bigint): {
    newPending: SenderTransactions;
    newQueued: SenderTransactions;
};
//# sourceMappingURL=reorganizeTransactionsLists.d.ts.map