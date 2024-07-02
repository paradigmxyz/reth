import test from 'ava';

import { deepEqual } from './deep-array';

test('depth 0', t => {
  t.true(deepEqual('a', 'a'));
  t.false(deepEqual('a', 'b'));
  t.false(deepEqual('a', ['a']));
  t.false(deepEqual(['a'], 'a'));
});

test('depth 1', t => {
  t.true(deepEqual(['a', 'b'], ['a', 'b']));
  t.false(deepEqual(['a'], ['a', 'b']));
  t.false(deepEqual(['a', 'b'], ['a', 'c']));
});

test('depth 2', t => {
  t.true(deepEqual([['a'], ['b']], [['a'], ['b']]));
  t.false(deepEqual([['a'], ['b']], [['a'], ['c']]));
});
