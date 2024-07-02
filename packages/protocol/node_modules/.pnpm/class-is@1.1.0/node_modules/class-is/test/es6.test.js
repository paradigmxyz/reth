'use strict';

const {
    Animal,
    Plant,
    Mammal,
    Algae,
} = require('./fixtures/es6');

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
