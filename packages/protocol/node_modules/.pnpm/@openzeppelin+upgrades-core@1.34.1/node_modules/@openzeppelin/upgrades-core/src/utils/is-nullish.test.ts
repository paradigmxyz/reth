import test from 'ava';

import { isNullish } from './is-nullish';

test('null', t => {
  t.true(isNullish(null));
});

test('undefined', t => {
  t.true(isNullish(undefined));
});

test('number 5', t => {
  t.false(isNullish(5));
});
