"use strict";

exports.__esModule = true;
exports["default"] = void 0;
/*!
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
/** @class */
var CognitoUserSession = exports["default"] = /*#__PURE__*/function () {
  /**
   * Constructs a new CognitoUserSession object
   * @param {CognitoIdToken} IdToken The session's Id token.
   * @param {CognitoRefreshToken=} RefreshToken The session's refresh token.
   * @param {CognitoAccessToken} AccessToken The session's access token.
   * @param {int} ClockDrift The saved computer's clock drift or undefined to force calculation.
   */
  function CognitoUserSession(_temp) {
    var _ref = _temp === void 0 ? {} : _temp,
      IdToken = _ref.IdToken,
      RefreshToken = _ref.RefreshToken,
      AccessToken = _ref.AccessToken,
      ClockDrift = _ref.ClockDrift;
    if (AccessToken == null || IdToken == null) {
      throw new Error('Id token and Access Token must be present.');
    }
    this.idToken = IdToken;
    this.refreshToken = RefreshToken;
    this.accessToken = AccessToken;
    this.clockDrift = ClockDrift === undefined ? this.calculateClockDrift() : ClockDrift;
  }

  /**
   * @returns {CognitoIdToken} the session's Id token
   */
  var _proto = CognitoUserSession.prototype;
  _proto.getIdToken = function getIdToken() {
    return this.idToken;
  }

  /**
   * @returns {CognitoRefreshToken} the session's refresh token
   */;
  _proto.getRefreshToken = function getRefreshToken() {
    return this.refreshToken;
  }

  /**
   * @returns {CognitoAccessToken} the session's access token
   */;
  _proto.getAccessToken = function getAccessToken() {
    return this.accessToken;
  }

  /**
   * @returns {int} the session's clock drift
   */;
  _proto.getClockDrift = function getClockDrift() {
    return this.clockDrift;
  }

  /**
   * @returns {int} the computer's clock drift
   */;
  _proto.calculateClockDrift = function calculateClockDrift() {
    var now = Math.floor(new Date() / 1000);
    var iat = Math.min(this.accessToken.getIssuedAt(), this.idToken.getIssuedAt());
    return now - iat;
  }

  /**
   * Checks to see if the session is still valid based on session expiry information found
   * in tokens and the current time (adjusted with clock drift)
   * @returns {boolean} if the session is still valid
   */;
  _proto.isValid = function isValid() {
    var now = Math.floor(new Date() / 1000);
    var adjusted = now - this.clockDrift;
    return adjusted < this.accessToken.getExpiration() && adjusted < this.idToken.getExpiration();
  };
  return CognitoUserSession;
}();