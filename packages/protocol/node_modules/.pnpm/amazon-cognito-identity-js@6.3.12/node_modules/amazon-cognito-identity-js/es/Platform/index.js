/*!
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
import { version } from './version';
var BASE_USER_AGENT = "aws-amplify/" + version;
export var Platform = {
  userAgent: BASE_USER_AGENT,
  isReactNative: typeof navigator !== 'undefined' && navigator.product === 'ReactNative'
};
export var getUserAgent = function getUserAgent() {
  return Platform.userAgent;
};

/**
 * @deprecated use named import
 */
export default Platform;