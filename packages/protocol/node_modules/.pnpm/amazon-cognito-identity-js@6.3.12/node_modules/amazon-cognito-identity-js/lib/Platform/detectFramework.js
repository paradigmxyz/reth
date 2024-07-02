"use strict";

exports.__esModule = true;
exports.detectFramework = void 0;
var _constants = require("./constants");
// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

var detectFramework = exports.detectFramework = function detectFramework() {
  if (typeof navigator !== 'undefined' && navigator.product === 'ReactNative') {
    return _constants.FRAMEWORK.ReactNative;
  }
  return _constants.FRAMEWORK.None;
};