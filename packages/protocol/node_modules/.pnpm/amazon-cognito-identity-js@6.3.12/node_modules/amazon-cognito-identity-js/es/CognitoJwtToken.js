/*!
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */

import { Buffer } from 'buffer';

/** @class */
var CognitoJwtToken = /*#__PURE__*/function () {
  /**
   * Constructs a new CognitoJwtToken object
   * @param {string=} token The JWT token.
   */
  function CognitoJwtToken(token) {
    // Assign object
    this.jwtToken = token || '';
    this.payload = this.decodePayload();
  }

  /**
   * @returns {string} the record's token.
   */
  var _proto = CognitoJwtToken.prototype;
  _proto.getJwtToken = function getJwtToken() {
    return this.jwtToken;
  }

  /**
   * @returns {int} the token's expiration (exp member).
   */;
  _proto.getExpiration = function getExpiration() {
    return this.payload.exp;
  }

  /**
   * @returns {int} the token's "issued at" (iat member).
   */;
  _proto.getIssuedAt = function getIssuedAt() {
    return this.payload.iat;
  }

  /**
   * @returns {object} the token's payload.
   */;
  _proto.decodePayload = function decodePayload() {
    var payload = this.jwtToken.split('.')[1];
    try {
      return JSON.parse(Buffer.from(payload, 'base64').toString('utf8'));
    } catch (err) {
      return {};
    }
  };
  return CognitoJwtToken;
}();
export { CognitoJwtToken as default };