/**
 * Writes data from the VM step to the in-memory
 * coverage map constructed by the Instrumenter.
 */
class DataCollector {
  constructor(instrumentationData={}){
    this.instrumentationData = instrumentationData;

    this.validOpcodes = {
      "PUSH1": true,
    }
  }

  /**
   * VM step event handler. Detects instrumentation hashes when they are pushed to the
   * top of the stack. This runs millions of times - trying to keep it fast.
   * @param  {Object} info  vm step info
   */
  step(info){
    try {
      if (this.validOpcodes[info.opcode.name] && info.stack.length > 0){
        const idx = info.stack.length - 1;
        let hash = '0x' +  info.stack[idx].toString(16);
        this._registerHash(hash)
      }
    } catch (err) { /*Ignore*/ };
  }

  // Temporarily disabled because some relevant traces aren't available
  /**
   * Converts pushData value to string and registers in instrumentation map.
   * @param  {HardhatEVMTraceInstruction} instruction
   */
  /*trackHardhatEVMInstruction(instruction){
    if (instruction && instruction.pushData){
      let hash = `0x` + instruction.pushData.toString('hex');
      this._registerHash(hash)
    }
  }*/

  /**
   * Normalizes has string and marks hit.
   * @param  {String} hash bytes32 hash
   */
  _registerHash(hash){
    hash = this._normalizeHash(hash);

    if(this.instrumentationData[hash]){
      this.instrumentationData[hash].hits++;
    }
  }

  /**
   * Left-pads zero prefixed bytes8 hashes to length 18. The '11' in the
   * comparison below is arbitrary. It provides a margin for recurring zeros
   * but prevents left-padding shorter irrelevant hashes
   *
   * @param  {String} hash  data hash from evm stack.
   * @return {String}       0x prefixed hash of length 66.
   */
  _normalizeHash(hash){
    if (hash.length < 18 && hash.length > 11){
      hash = hash.slice(2);
      while(hash.length < 16) hash = '0' + hash;
      hash = '0x' + hash
    }
    return hash;
  }

  /**
   * Unit test helper
   * @param {Object} data  Instrumenter.instrumentationData
   */
  _setInstrumentationData(data){
    this.instrumentationData = data;
  }
}

module.exports = DataCollector;