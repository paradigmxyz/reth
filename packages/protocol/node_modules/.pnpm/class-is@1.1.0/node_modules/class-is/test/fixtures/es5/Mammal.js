'use strict';

const withIs = require('../../..');
const Animal = require('./Animal');

function Mammal() {
    Animal.call(this, 'mammal');
}

Mammal.prototype = Object.create(Animal.prototype);
Mammal.prototype.constructor = Mammal;

module.exports = withIs.proto(Mammal, {
    className: 'Mammal',
    symbolName: '@org/package/Mammal',
});
module.exports.WrappedClass = Mammal;
