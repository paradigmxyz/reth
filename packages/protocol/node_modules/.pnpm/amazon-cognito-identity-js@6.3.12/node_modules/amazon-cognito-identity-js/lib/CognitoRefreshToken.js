"use strict";

exports.__esModule = true;
exports["default"] = void 0;
/*!
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
/** @class */
var CognitoRefreshToken = exports["default"] = /*#__PURE__*/function () {
  /**
   * Constructs a new CognitoRefreshToken object
   * @param {string=} RefreshToken The JWT refresh token.
   */
  function CognitoRefreshToken(_temp) {
    var _ref = _temp === void 0 ? {} : _temp,
      RefreshToken = _ref.RefreshToken;
    // Assign object
    this.token = RefreshToken || '';
  }

  /**
   * @returns {string} the record's token.
   */
  var _proto = CognitoRefreshToken.prototype;
  _proto.getToken = function getToken() {
    return this.token;
  };
  return CognitoRefreshToken;
}();