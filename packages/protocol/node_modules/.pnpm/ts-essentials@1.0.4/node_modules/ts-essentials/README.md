<p align="center">
  <img src="https://emojipedia-us.s3.dualstack.us-west-1.amazonaws.com/thumbs/240/google/146/toolbox_1f9f0.png" width="120" alt="TypeStrict">
  <h3 align="center">ts-essentials</h3>
  <p align="center">All essential TypeScript types in one place 🤙</p>
  <p align="center">
    <img alt="Downloads" src="https://img.shields.io/npm/dm/ts-essentials.svg">
    <img alt="Build status" src="https://circleci.com/gh/krzkaczor/ts-essentials.svg?style=svg">
    <a href="/package.json"><img alt="Software License" src="https://img.shields.io/badge/license-MIT-brightgreen.svg?style=flat-square"></a>
  </p>
</p>

## Install

```sh
npm add --save-dev ts-essentials
```

or

```sh
yarn add --dev ts-essentials
```

## What's inside?

- [Basic](#basic)
  - Primitive
  - NonNullable
- [Dictionaries](#dictionaries)
  - Dictionary
  - DictionaryValues
- [Deep Partial & DeepRequired & Deep Readonly](#deep-partial--deep-required--deep-readonly)
- [Omit](#omit)
- [Merge](#merge)
- [Opaque types](#opaque-types)
- [Literal types](#literal-types)
- [Exhaustive switch cases](#exhaustive-switch-cases)
- [ValueOf](#valueof-type)

### Basic:

- `Primitive` type matching all primitive values.
- `NonNullable` remove `null` and `undefined` from union type.

### Dictionaries

```typescript
const stringDict: Dictionary<string> = {
  a: "A",
  b: "B",
};

// Specify second type argument to change dictionary keys type
const dictOfNumbers: Dictionary<string, number> = {
  420: "four twenty",
  1337: "HAX",
};

// You may specify union types as key to cover all possible cases. It acts the same as Record from TS's standard library
export type DummyOptions = "open" | "closed" | "unknown";
const dictFromUnionType: Dictionary<number, DummyOptions> = {
  closed: 1,
  open: 2,
  unknown: 3,
};

// and get dictionary values
type stringDictValues = DictionaryValues<typeof stringDict>;
```

### Deep Partial & Deep Required & Deep Readonly

```typescript
type ComplexObject = {
  simple: number;
  nested: {
    a: string;
    array: [{ bar: number }];
  };
};

type ComplexObjectPartial = DeepPartial<ComplexObject>;
const samplePartial: ComplexObjectPartial = {
  nested: {
    array: [{}],
  },
};

type ComplexObjectAgain = DeepRequired<ComplexObjectPartial>;
const sampleRequired: ComplexObjectAgain = {
  simple: 5,
  nested: {
    a: "test",
    array: [],
  },
};

type ComplexObjectReadonly = DeepReadonly<ComplexObject>;
```

### Omit

```typescript
type SimplifiedComplexObject = Omit<ComplexObject, "nested">;
```

### Merge

```typescript
type Foo = {
  a: number,
  b: string
};

type Bar = {
  b: number
};

const xyz: Merge<Foo, Bar> = { a: 4, b: 2 };
```

### Opaque types

```typescript
type PositiveNumber = Opaque<number, "positive-number">;

function makePositiveNumber(n: number): PositiveNumber {
  if (n <= 0) {
    throw new Error("Value not positive !!!");
  }
  return (n as any) as PositiveNumber; // this ugly cast is required but only when "producing" opaque types
}
```

### Literal types

```typescript
// prevent type widening https://blog.mariusschulz.com/2017/02/04/typescript-2-1-literal-type-widening
const t = {
  letter: literal("a"), // type stays "a" not string
  digit: literal(5), // type stays 5 not number
};
```

### Exhaustive switch cases

```typescript
function actOnDummyOptions(options: DummyOptions): string {
  switch (options) {
    case "open":
      return "it's open!";
    case "closed":
      return "it's closed";
    case "unknown":
      return "i have no idea";
    default:
      // if you would add another option to DummyOptions, you'll get error here!
      throw new UnreachableCaseError(options);
  }
}
```

### ValueOf type

```typescript
const obj = {
  id: "123e4567-e89b-12d3-a456-426655440000",
  name: "Test object",
  timestamp: 1548768231486,
};

type objKeys = ValueOf<typeof obj>; // string | number
```


## Contributing

All contributions are welcomed [Read more](./CONTRIBUTING.md)