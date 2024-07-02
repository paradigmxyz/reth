import { Block } from '@nomicfoundation/ethereumjs-block'
import { ConsensusType } from '@nomicfoundation/ethereumjs-common'
import { RLP } from '@nomicfoundation/ethereumjs-rlp'
import { Trie } from '@nomicfoundation/ethereumjs-trie'
import { Address, TypeOutput, toBuffer, toType } from '@nomicfoundation/ethereumjs-util'

import { Bloom } from './bloom'
import { calculateMinerReward, encodeReceipt, rewardAccount } from './runBlock'

import type { BuildBlockOpts, BuilderOpts, RunTxResult, SealBlockOpts } from './types'
import type { VM } from './vm'
import type { HeaderData } from '@nomicfoundation/ethereumjs-block'
import type { TypedTransaction } from '@nomicfoundation/ethereumjs-tx'

export class BlockBuilder {
  /**
   * The cumulative gas used by the transactions added to the block.
   */
  gasUsed = BigInt(0)

  private readonly vm: VM
  private blockOpts: BuilderOpts
  private headerData: HeaderData
  private transactions: TypedTransaction[] = []
  private transactionResults: RunTxResult[] = []
  private checkpointed = false
  private reverted = false
  private built = false

  get transactionReceipts() {
    return this.transactionResults.map((result) => result.receipt)
  }

  constructor(vm: VM, opts: BuildBlockOpts) {
    this.vm = vm
    this.blockOpts = { putBlockIntoBlockchain: true, ...opts.blockOpts, common: this.vm._common }

    this.headerData = {
      ...opts.headerData,
      parentHash: opts.headerData?.parentHash ?? opts.parentBlock.hash(),
      number: opts.headerData?.number ?? opts.parentBlock.header.number + BigInt(1),
      gasLimit: opts.headerData?.gasLimit ?? opts.parentBlock.header.gasLimit,
    }

    if (
      this.vm._common.isActivatedEIP(1559) === true &&
      typeof this.headerData.baseFeePerGas === 'undefined'
    ) {
      this.headerData.baseFeePerGas = opts.parentBlock.header.calcNextBaseFee()
    }
  }

  /**
   * Throws if the block has already been built or reverted.
   */
  private checkStatus() {
    if (this.built) {
      throw new Error('Block has already been built')
    }
    if (this.reverted) {
      throw new Error('State has already been reverted')
    }
  }

  /**
   * Calculates and returns the transactionsTrie for the block.
   */
  private async transactionsTrie() {
    const trie = new Trie()
    for (const [i, tx] of this.transactions.entries()) {
      await trie.put(Buffer.from(RLP.encode(i)), tx.serialize())
    }
    return trie.root()
  }

  /**
   * Calculates and returns the logs bloom for the block.
   */
  private logsBloom() {
    const bloom = new Bloom()
    for (const txResult of this.transactionResults) {
      // Combine blooms via bitwise OR
      bloom.or(txResult.bloom)
    }
    return bloom.bitvector
  }

  /**
   * Calculates and returns the receiptTrie for the block.
   */
  private async receiptTrie() {
    const receiptTrie = new Trie()
    for (const [i, txResult] of this.transactionResults.entries()) {
      const tx = this.transactions[i]
      const encodedReceipt = encodeReceipt(txResult.receipt, tx.type)
      await receiptTrie.put(Buffer.from(RLP.encode(i)), encodedReceipt)
    }
    return receiptTrie.root()
  }

  /**
   * Adds the block miner reward to the coinbase account.
   */
  private async rewardMiner() {
    const minerReward = this.vm._common.param('pow', 'minerReward')
    const reward = calculateMinerReward(minerReward, 0)
    const coinbase =
      this.headerData.coinbase !== undefined
        ? new Address(toBuffer(this.headerData.coinbase))
        : Address.zero()
    await rewardAccount(this.vm.eei, coinbase, reward)
  }

  /**
   * Run and add a transaction to the block being built.
   * Please note that this modifies the state of the VM.
   * Throws if the transaction's gasLimit is greater than
   * the remaining gas in the block.
   */
  async addTransaction(tx: TypedTransaction) {
    this.checkStatus()

    if (!this.checkpointed) {
      await this.vm.stateManager.checkpoint()
      this.checkpointed = true
    }

    // According to the Yellow Paper, a transaction's gas limit
    // cannot be greater than the remaining gas in the block
    const blockGasLimit = toType(this.headerData.gasLimit, TypeOutput.BigInt)
    const blockGasRemaining = blockGasLimit - this.gasUsed
    if (tx.gasLimit > blockGasRemaining) {
      throw new Error('tx has a higher gas limit than the remaining gas in the block')
    }

    const header = {
      ...this.headerData,
      gasUsed: this.gasUsed,
    }
    const blockData = { header, transactions: this.transactions }
    const block = Block.fromBlockData(blockData, this.blockOpts)

    const result = await this.vm.runTx({ tx, block })

    this.transactions.push(tx)
    this.transactionResults.push(result)
    this.gasUsed += result.totalGasSpent

    return result
  }

  /**
   * Reverts the checkpoint on the StateManager to reset the state from any transactions that have been run.
   */
  async revert() {
    this.checkStatus()
    if (this.checkpointed) {
      await this.vm.stateManager.revert()
      this.reverted = true
    }
  }

  /**
   * This method returns the finalized block.
   * It also:
   *  - Assigns the reward for miner (PoW)
   *  - Commits the checkpoint on the StateManager
   *  - Sets the tip of the VM's blockchain to this block
   * For PoW, optionally seals the block with params `nonce` and `mixHash`,
   * which is validated along with the block number and difficulty by ethash.
   * For PoA, please pass `blockOption.cliqueSigner` into the buildBlock constructor,
   * as the signer will be awarded the txs amount spent on gas as they are added.
   */
  async build(sealOpts?: SealBlockOpts) {
    this.checkStatus()
    const blockOpts = this.blockOpts
    const consensusType = this.vm._common.consensusType()

    if (consensusType === ConsensusType.ProofOfWork) {
      await this.rewardMiner()
    }

    const stateRoot = await this.vm.stateManager.getStateRoot()
    const transactionsTrie = await this.transactionsTrie()
    const receiptTrie = await this.receiptTrie()
    const logsBloom = this.logsBloom()
    const gasUsed = this.gasUsed
    const timestamp = this.headerData.timestamp ?? Math.round(Date.now() / 1000)

    const headerData = {
      ...this.headerData,
      stateRoot,
      transactionsTrie,
      receiptTrie,
      logsBloom,
      gasUsed,
      timestamp,
    }

    if (consensusType === ConsensusType.ProofOfWork) {
      headerData.nonce = sealOpts?.nonce ?? headerData.nonce
      headerData.mixHash = sealOpts?.mixHash ?? headerData.mixHash
    }

    const blockData = { header: headerData, transactions: this.transactions }
    const block = Block.fromBlockData(blockData, blockOpts)

    if (this.blockOpts.putBlockIntoBlockchain === true) {
      await this.vm.blockchain.putBlock(block)
    }

    this.built = true
    if (this.checkpointed) {
      await this.vm.stateManager.commit()
      this.checkpointed = false
    }

    return block
  }
}

export async function buildBlock(this: VM, opts: BuildBlockOpts): Promise<BlockBuilder> {
  return new BlockBuilder(this, opts)
}
