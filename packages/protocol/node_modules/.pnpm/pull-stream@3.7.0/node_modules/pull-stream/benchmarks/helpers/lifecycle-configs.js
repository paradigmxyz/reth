const prettyBytes = require('pretty-bytes');
const gc = require('expose-gc/function');

function getLifecycleConfigs(notCountedFn) {
  let countMem = 0;
  let streamingTotalMem = 0;
  let streamingMeanMem = 0;
  let streamingMaxMem = 0;
  let streamingMinMem = Number.MAX_VALUE;
  let streamingVarianceTotal = 0;
  let oldMemory;
  function calcMemUsage() {
    const newMemory = process.memoryUsage().heapUsed;
    if (countMem === 0) {
      oldMemory = newMemory;
      countMem++;
    } else {
      const memDiff = newMemory - oldMemory;
      streamingMaxMem = Math.max(streamingMaxMem, memDiff);
      streamingMinMem = Math.min(streamingMinMem, memDiff);
      streamingTotalMem += memDiff;
      const newMean = streamingTotalMem / countMem;
      streamingVarianceTotal += (memDiff - newMean) * (memDiff - streamingMeanMem);
      streamingMeanMem = newMean;
      countMem++;
      oldMemory = newMemory;
    }
  }

  function onCycle() {
    calcMemUsage();
    notCountedFn && notCountedFn();
    gc();
  }
  function onStart() {
    countMem = 0;
    streamingTotalMem = 0;
    streamingMeanMem = 0;
    streamingMaxMem = 0;
    streamingMinMem = Number.MAX_VALUE;
    streamingVarianceTotal = 0;
    gc();
    onCycle();
  }
  function onComplete(event) {
    console.log();
    console.log(event.target.toString());
    console.log('Heap Change - ' +
      'max: ' + prettyBytes(streamingMaxMem) +
      ', min:' + prettyBytes(streamingMinMem) +
      ', mean:' + prettyBytes(streamingMeanMem) +
      ', std dev:' + prettyBytes(Math.sqrt(streamingVarianceTotal / (countMem - 1)))
    );
  }

  return {
    onStart: onStart,
    onCycle: onCycle,
    onComplete: onComplete
  };
}

module.exports = getLifecycleConfigs;