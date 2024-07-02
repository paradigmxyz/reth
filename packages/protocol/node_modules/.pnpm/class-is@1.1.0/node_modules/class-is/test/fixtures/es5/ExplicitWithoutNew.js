'use strict';

const withIs = require('../../..');

function ExplicitWithoutNew_() {
    if (!(this instanceof ExplicitWithoutNew)) {
        return new ExplicitWithoutNew();
    }

    this.label = 'ExplicitWithoutNew';
}

ExplicitWithoutNew_.prototype.getLabel = function () {
    return this.label;
};

const ExplicitWithoutNew = withIs.proto(ExplicitWithoutNew_, {
    className: 'ExplicitWithoutNew',
    symbolName: '@org/package/ExplicitWithoutNew',
});

module.exports = ExplicitWithoutNew;
module.exports.WrappedClass = ExplicitWithoutNew_;
