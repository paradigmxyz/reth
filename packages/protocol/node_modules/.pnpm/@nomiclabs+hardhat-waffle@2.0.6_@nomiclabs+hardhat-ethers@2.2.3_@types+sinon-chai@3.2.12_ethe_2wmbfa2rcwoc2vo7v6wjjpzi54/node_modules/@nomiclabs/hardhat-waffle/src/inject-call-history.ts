import type { RecordedCall } from "@ethereum-waffle/provider";
import { utils } from "ethers";

/**
 * Injects call history into hardhat provider,
 * making it possible to use matchers like calledOnContract
 */

class CallHistory {
  public recordedCalls: RecordedCall[] = [];

  public addUniqueCall(call: RecordedCall) {
    if (
      this.recordedCalls.find(
        (c) => c.address === call.address && c.data === call.data
      ) === undefined
    ) {
      this.recordedCalls.push(call);
    }
  }

  public clearAll() {
    this.recordedCalls = [];
  }
}

function toRecordedCall(message: any): RecordedCall {
  return {
    address: message.to?.buf
      ? utils.getAddress(utils.hexlify(message.to.buf))
      : undefined,
    data: message.data ? utils.hexlify(message.data) : "0x",
  };
}

function getHardhatVMEventEmitter(hardhatWaffleProvider: any) {
  const vm =
    hardhatWaffleProvider?._hardhatNetwork.provider?._wrapped._wrapped?._wrapped
      ?._node?._vmTracer?._vm;

  /**
   * There were changes related to the location of event emitter introduced
   * in Hardhat version 2.11.0.
   */
  return vm?.evm?.events ?? vm;
}

let injected = false;

export const injectCallHistory = (hardhatWaffleProvider: any) => {
  if (injected) {
    return;
  }

  const callHistory = new CallHistory();
  hardhatWaffleProvider.clearCallHistory = () => {
    callHistory.clearAll();
  };

  let beforeMessageListener: (message: any) => void | undefined;
  const init =
    hardhatWaffleProvider?._hardhatNetwork?.provider?._wrapped?._wrapped
      ?._wrapped?._init;
  if (!init) {
    throw new Error("Could not inject call history into hardhat provider");
  }
  hardhatWaffleProvider._hardhatNetwork.provider._wrapped._wrapped._wrapped._init =
    async function () {
      await init.apply(this);
      if (typeof beforeMessageListener === "function") {
        // has to be here because of weird behaviour of init function, which requires us to re-register the handler.
        getHardhatVMEventEmitter(hardhatWaffleProvider)?.off?.(
          "beforeMessage",
          beforeMessageListener
        );
      }
      beforeMessageListener = (message: any) => {
        callHistory.addUniqueCall(toRecordedCall(message));
      };
      hardhatWaffleProvider.callHistory = callHistory.recordedCalls;
      getHardhatVMEventEmitter(hardhatWaffleProvider)?.on?.(
        "beforeMessage",
        beforeMessageListener
      );
    };

  injected = true;
};
