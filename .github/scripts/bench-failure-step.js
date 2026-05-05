// Shared failure step detection for PR comments and Slack notifications.

function failedStep({ bigBlocks, abba, outcomes }) {
  const steps = [
    ...(bigBlocks ? [['validating local big-block data', outcomes.bigBlocksCheck]] : []),
    ['validating local snapshot', outcomes.snapshotCheck],
    ['building binaries', outcomes.build],
    ['running feature benchmark (1/2)', outcomes.runFeature1],
    ['running baseline benchmark (1/2)', outcomes.runBaseline1],
    ...(abba ? [['running baseline benchmark (2/2)', outcomes.runBaseline2]] : []),
    ...(abba ? [['running feature benchmark (2/2)', outcomes.runFeature2]] : []),
  ];
  const failed = steps.find(([, outcome]) => outcome === 'failure');
  return failed ? failed[0] : 'unknown step';
}

module.exports = { failedStep };
