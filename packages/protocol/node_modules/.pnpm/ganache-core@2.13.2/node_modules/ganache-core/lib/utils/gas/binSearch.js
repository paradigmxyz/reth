const { BN } = require("ethereumjs-util");
const hexToBn = (val = 0) => new BN(parseInt("0x" + val.toString("hex"), 16));
const MULTIPLE = 64 / 63;

module.exports = async function binSearch(generateVM, runArgs, result, callback) {
  const MAX = hexToBn(runArgs.block.header.gasLimit);
  const gasRefund = result.execResult.gasRefund;
  const startingGas = gasRefund ? result.gasEstimate.add(gasRefund) : result.gasEstimate;
  const range = { lo: startingGas, hi: startingGas };
  const isEnoughGas = async(gas) => {
    const vm = generateVM(); // Generate fresh VM
    runArgs.tx.gasLimit = gas.toBuffer();
    const result = await vm.runTx(runArgs).catch((vmerr) => ({ vmerr }));
    return !result.vmerr && !result.execResult.exceptionError;
  };

  if (!(await isEnoughGas(range.hi))) {
    do {
      range.hi = range.hi.muln(MULTIPLE);
    } while (!(await isEnoughGas(range.hi)));
    while (range.lo.addn(1).lt(range.hi)) {
      const mid = range.lo.add(range.hi).divn(2);
      if (await isEnoughGas(mid)) {
        range.hi = mid;
      } else {
        range.lo = mid;
      }
    }
    if (range.hi.gte(MAX)) {
      if (!(await isEnoughGas(range.hi))) {
        return callback(new Error("gas required exceeds allowance or always failing transaction"));
      }
    }
  }

  result.gasEstimate = range.hi;
  callback(null, result);
};
