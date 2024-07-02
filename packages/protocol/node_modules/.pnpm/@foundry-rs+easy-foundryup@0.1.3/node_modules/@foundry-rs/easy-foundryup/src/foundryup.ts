#!/usr/bin/env node
"use strict";

import { run } from "./index";

void (async () => {
  const success = await run();
  if (success) {
    console.log("successfully setup foundryup");
  } else {
    console.log("failed to setup foundryup");
    process.exit(1);
  }
})();
