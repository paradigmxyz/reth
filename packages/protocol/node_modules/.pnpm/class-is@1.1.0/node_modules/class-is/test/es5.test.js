'use strict';

const {
    Animal,
    Plant,
    Mammal,
    Algae,

    ExplicitWithoutNew,
    ImplicitWithoutNew,
    ImplicitExplicitWithoutNew,
} = require('./fixtures/es5');

it('should setup the prototype chain correctly', () => {
    const animal = new Animal('mammal');
    const plant = new Plant('algae');

    expect(Object.getPrototypeOf(animal)).toBe(Animal.prototype);
    expect(Object.getPrototypeOf(Object.getPrototypeOf(animal))).toBe(Animal.WrappedClass.prototype);
    expect(Object.getPrototypeOf(animal)).not.toBe(Plant.prototype);
    expect(Object.getPrototypeOf(plant)).toBe(Plant.prototype);
    expect(Object.getPrototypeOf(Object.getPrototypeOf(plant))).toBe(Plant.WrappedClass.prototype);
    expect(Object.getPrototypeOf(plant)).not.toBe(Animal.prototype);

    expect(animal instanceof Animal).toBe(true);
    expect(animal instanceof Animal.WrappedClass).toBe(true);
    expect(animal instanceof Plant).toBe(false);
    expect(plant instanceof Plant).toBe(true);
    expect(plant instanceof Plant.WrappedClass).toBe(true);
    expect(plant instanceof Animal).toBe(false);

    expect(animal.getType()).toBe('mammal');
    expect(plant.getType()).toBe('algae');
});

it('should have a custom toStringTag', () => {
    expect(Object.prototype.toString.call(new Animal())).toBe('[object Animal]');
    expect(Object.prototype.toString.call(new Plant())).toBe('[object Plant]');
});

describe('is<className> method', () => {
    it('should add a working is<className> static method', () => {
        const animal = new Animal('mammal');
        const plant = new Plant('algae');

        expect(Animal.isAnimal(animal)).toBe(true);
        expect(Animal.isAnimal(plant)).toBe(false);
        expect(Plant.isPlant(plant)).toBe(true);
        expect(Plant.isPlant(animal)).toBe(false);
    });

    it('should not crash if `null` or `undefined` is passed to is<ClassName>', () => {
        expect(Animal.isAnimal(null)).toBe(false);
        expect(Animal.isAnimal(undefined)).toBe(false);
    });

    it('should work correctly for deep inheritance scenarios', () => {
        const mammal = new Mammal();
        const algae = new Algae();

        expect(Mammal.isMammal(mammal)).toBe(true);
        expect(Animal.isAnimal(mammal)).toBe(true);
        expect(Mammal.isMammal(algae)).toBe(false);
        expect(Animal.isAnimal(algae)).toBe(false);

        expect(Algae.isAlgae(algae)).toBe(true);
        expect(Plant.isPlant(algae)).toBe(true);
        expect(Algae.isAlgae(mammal)).toBe(false);
        expect(Plant.isPlant(mammal)).toBe(false);
    });
});

describe('new operator', () => {
    it('should work on explicit without-new handling', () => {
        const instance = new ExplicitWithoutNew();
        const instance2 = ExplicitWithoutNew(); // eslint-disable-line new-cap

        expect(Object.getPrototypeOf(instance)).toBe(ExplicitWithoutNew.prototype);
        expect(Object.getPrototypeOf(Object.getPrototypeOf(instance))).toBe(ExplicitWithoutNew.WrappedClass.prototype);
        expect(Object.getPrototypeOf(instance2)).toBe(ExplicitWithoutNew.prototype);
        expect(Object.getPrototypeOf(Object.getPrototypeOf(instance2))).toBe(ExplicitWithoutNew.WrappedClass.prototype);

        expect(instance instanceof ExplicitWithoutNew).toBe(true);
        expect(instance instanceof ExplicitWithoutNew.WrappedClass).toBe(true);
        expect(instance2 instanceof ExplicitWithoutNew).toBe(true);
        expect(instance2 instanceof ExplicitWithoutNew.WrappedClass).toBe(true);

        expect(instance.getLabel()).toBe('ExplicitWithoutNew');
        expect(instance2.getLabel()).toBe('ExplicitWithoutNew');
    });

    it('should work on implicit without-new handling', () => {
        const instance = new ImplicitWithoutNew();
        const instanceNoNew = ImplicitWithoutNew(); // eslint-disable-line new-cap

        expect(Object.getPrototypeOf(instance)).toBe(ImplicitWithoutNew.prototype);
        expect(Object.getPrototypeOf(Object.getPrototypeOf(instance))).toBe(ImplicitWithoutNew.WrappedClass.prototype);
        expect(Object.getPrototypeOf(instanceNoNew)).toBe(ImplicitWithoutNew.prototype);
        expect(Object.getPrototypeOf(Object.getPrototypeOf(instanceNoNew))).toBe(ImplicitWithoutNew.WrappedClass.prototype);

        expect(instance instanceof ImplicitWithoutNew).toBe(true);
        expect(instance instanceof ImplicitWithoutNew.WrappedClass).toBe(true);
        expect(instanceNoNew instanceof ImplicitWithoutNew).toBe(true);
        expect(instanceNoNew instanceof ImplicitWithoutNew.WrappedClass).toBe(true);

        expect(instance.getLabel()).toBe('ImplicitWithoutNew');
        expect(instanceNoNew.getLabel()).toBe('ImplicitWithoutNew');
    });

    it('should work on explicit & implicit without-new handling', () => {
        const instance = new ImplicitExplicitWithoutNew();
        const instanceNoNew = ImplicitExplicitWithoutNew(); // eslint-disable-line new-cap

        expect(Object.getPrototypeOf(instance)).toBe(ImplicitExplicitWithoutNew.prototype);
        expect(Object.getPrototypeOf(Object.getPrototypeOf(instance))).toBe(ImplicitExplicitWithoutNew.WrappedClass.prototype);
        expect(Object.getPrototypeOf(instanceNoNew)).toBe(ImplicitExplicitWithoutNew.prototype);
        expect(Object.getPrototypeOf(Object.getPrototypeOf(instanceNoNew))).toBe(ImplicitExplicitWithoutNew.WrappedClass.prototype);

        expect(instance instanceof ImplicitExplicitWithoutNew).toBe(true);
        expect(instance instanceof ImplicitExplicitWithoutNew.WrappedClass).toBe(true);
        expect(instanceNoNew instanceof ImplicitExplicitWithoutNew).toBe(true);
        expect(instanceNoNew instanceof ImplicitExplicitWithoutNew.WrappedClass).toBe(true);

        expect(instance.getLabel()).toBe('ImplicitExplicitWithoutNew');
        expect(instanceNoNew.getLabel()).toBe('ImplicitExplicitWithoutNew');
    });
});
