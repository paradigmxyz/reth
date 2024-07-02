import test from 'ava';

import { levenshtein } from './levenshtein';

const match = <T>(a: T, b: T) => (a === b ? undefined : { kind: 'replace', original: a, updated: b });

test('equal', t => {
  const a = [...'abc'];
  const b = [...'abc'];
  const ops = levenshtein(a, b, match);
  t.deepEqual(ops, []);
});

test('append', t => {
  const a = [...'abc'];
  const b = [...'abcd'];
  const ops = levenshtein(a, b, match);
  t.like(ops, {
    length: 1,
    0: {
      kind: 'append',
      updated: 'd',
    },
  });
});

test('delete from end', t => {
  const a = [...'abcd'];
  const b = [...'abc'];
  const ops = levenshtein(a, b, match);
  t.like(ops, {
    length: 1,
    0: {
      kind: 'delete',
      original: 'd',
    },
  });
});

test('delete from middle', t => {
  const a = [...'abc'];
  const b = [...'ac'];
  const ops = levenshtein(a, b, match);
  t.like(ops, {
    length: 1,
    0: {
      kind: 'delete',
      original: 'b',
    },
  });
});

test('delete from beginning', t => {
  const a = [...'abc'];
  const b = [...'bc'];
  const ops = levenshtein(a, b, match);
  t.like(ops, {
    length: 1,
    0: {
      kind: 'delete',
      original: 'a',
    },
  });
});

test('insert', t => {
  const a = [...'abc'];
  const b = [...'azbc'];
  const ops = levenshtein(a, b, match);
  t.like(ops, {
    length: 1,
    0: {
      kind: 'insert',
      updated: 'z',
    },
  });
});

test('replace', t => {
  const a = [...'abc'];
  const b = [...'axc'];
  const ops = levenshtein(a, b, match);
  t.like(ops, {
    length: 1,
    0: {
      kind: 'replace',
      original: 'b',
      updated: 'x',
    },
  });
});
