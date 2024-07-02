"use strict";

exports.__esModule = true;
exports["default"] = void 0;
/*!
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
/** @class */
var AuthenticationDetails = exports["default"] = /*#__PURE__*/function () {
  /**
   * Constructs a new AuthenticationDetails object
   * @param {object=} data Creation options.
   * @param {string} data.Username User being authenticated.
   * @param {string} data.Password Plain-text password to authenticate with.
   * @param {(AttributeArg[])?} data.ValidationData Application extra metadata.
   * @param {(AttributeArg[])?} data.AuthParamaters Authentication paramaters for custom auth.
   */
  function AuthenticationDetails(data) {
    var _ref = data || {},
      ValidationData = _ref.ValidationData,
      Username = _ref.Username,
      Password = _ref.Password,
      AuthParameters = _ref.AuthParameters,
      ClientMetadata = _ref.ClientMetadata;
    this.validationData = ValidationData || {};
    this.authParameters = AuthParameters || {};
    this.clientMetadata = ClientMetadata || {};
    this.username = Username;
    this.password = Password;
  }

  /**
   * @returns {string} the record's username
   */
  var _proto = AuthenticationDetails.prototype;
  _proto.getUsername = function getUsername() {
    return this.username;
  }

  /**
   * @returns {string} the record's password
   */;
  _proto.getPassword = function getPassword() {
    return this.password;
  }

  /**
   * @returns {Array} the record's validationData
   */;
  _proto.getValidationData = function getValidationData() {
    return this.validationData;
  }

  /**
   * @returns {Array} the record's authParameters
   */;
  _proto.getAuthParameters = function getAuthParameters() {
    return this.authParameters;
  }

  /**
   * @returns {ClientMetadata} the clientMetadata for a Lambda trigger
   */;
  _proto.getClientMetadata = function getClientMetadata() {
    return this.clientMetadata;
  };
  return AuthenticationDetails;
}();