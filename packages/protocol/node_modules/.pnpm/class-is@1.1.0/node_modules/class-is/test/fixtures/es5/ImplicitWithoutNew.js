'use strict';

const withIs = require('../../..');

function ImplicitWithoutNew() {
    this.label = 'ImplicitWithoutNew';
}

ImplicitWithoutNew.prototype.getLabel = function () {
    return this.label;
};

module.exports = withIs.proto(ImplicitWithoutNew, {
    className: 'ImplicitWithoutNew',
    symbolName: '@org/package/ImplicitWithoutNew',
    withoutNew: true,
});
module.exports.WrappedClass = ImplicitWithoutNew;
