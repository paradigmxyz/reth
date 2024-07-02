import { getUserAgent } from './Platform';
import { AUTH_CATEGORY } from './Platform/constants';

// constructor
function UserAgent() {}
// public
UserAgent.prototype.userAgent = getUserAgent();
export var appendToCognitoUserAgent = function appendToCognitoUserAgent(content) {
  if (!content) {
    return;
  }
  if (UserAgent.prototype.userAgent && !UserAgent.prototype.userAgent.includes(content)) {
    UserAgent.prototype.userAgent = UserAgent.prototype.userAgent.concat(' ', content);
  }
  if (!UserAgent.prototype.userAgent || UserAgent.prototype.userAgent === '') {
    UserAgent.prototype.userAgent = content;
  }
};
export var addAuthCategoryToCognitoUserAgent = function addAuthCategoryToCognitoUserAgent() {
  UserAgent.category = AUTH_CATEGORY;
};
export var addFrameworkToCognitoUserAgent = function addFrameworkToCognitoUserAgent(framework) {
  UserAgent.framework = framework;
};
export var getAmplifyUserAgent = function getAmplifyUserAgent(action) {
  var uaCategoryAction = UserAgent.category ? " " + UserAgent.category : '';
  var uaFramework = UserAgent.framework ? " framework/" + UserAgent.framework : '';
  var userAgent = "" + UserAgent.prototype.userAgent + uaCategoryAction + uaFramework;
  return userAgent;
};

// class for defining the amzn user-agent
export default UserAgent;