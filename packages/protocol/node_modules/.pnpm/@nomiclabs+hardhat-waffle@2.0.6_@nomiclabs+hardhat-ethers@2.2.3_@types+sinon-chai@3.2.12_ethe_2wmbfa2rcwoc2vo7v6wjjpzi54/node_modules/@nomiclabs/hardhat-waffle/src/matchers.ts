export function initializeWaffleMatchers(projectRoot: string) {
  try {
    // We use the projectRoot to guarantee that we are using the user's
    // installed version of chai
    const chaiPath = require.resolve("chai", {
      paths: [projectRoot],
    });

    const chai = require(chaiPath);
    // eslint-disable-next-line import/no-extraneous-dependencies
    const { waffleChai } = require("@ethereum-waffle/chai");

    chai.use(waffleChai);
  } catch {
    // If chai isn't installed we just don't initialize the matchers
  }
}
