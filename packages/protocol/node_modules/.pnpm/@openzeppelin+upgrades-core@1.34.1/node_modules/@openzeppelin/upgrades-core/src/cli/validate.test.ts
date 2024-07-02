import test from 'ava';

import minimist from 'minimist';
import { getFunctionArgs, withDefaults } from './validate';
import sinon from 'sinon';
import { errorKinds } from '../validate/run';

test.afterEach.always(() => {
  sinon.restore();
});

test('getFunctionArgs - invalid command', t => {
  const parsedArgs = minimist(['invalid']);
  const extraArgs = parsedArgs._;
  t.throws(() => getFunctionArgs(parsedArgs, extraArgs), {
    message: `Invalid command: invalid. Supported commands are: validate`,
  });
});

test('getFunctionArgs - invalid options', async t => {
  const parsedArgs = minimist(['validate', 'build-info.json', '--foo', '--bar', 'xyz']);
  const extraArgs = parsedArgs._;
  t.throws(() => getFunctionArgs(parsedArgs, extraArgs), {
    message: `Invalid options: foo, bar`,
  });
});

test('getFunctionArgs - command only', t => {
  const parsedArgs = minimist(['validate']);
  const extraArgs = parsedArgs._;
  const functionArgs = getFunctionArgs(parsedArgs, extraArgs);
  if (functionArgs === undefined) {
    t.fail();
  } else {
    t.is(functionArgs.buildInfoDir, undefined);
    t.is(functionArgs.opts.unsafeAllowRenames, false);
    t.is(functionArgs.opts.unsafeSkipStorageCheck, false);
    t.is(functionArgs.opts.unsafeAllowCustomTypes, false);
    t.is(functionArgs.opts.unsafeAllowLinkedLibraries, false);
    t.deepEqual(functionArgs.opts.unsafeAllow, []);
  }
});

test('getFunctionArgs - command with arg', t => {
  const parsedArgs = minimist(['validate', 'build-info.json']);
  const extraArgs = parsedArgs._;
  const functionArgs = getFunctionArgs(parsedArgs, extraArgs);
  if (functionArgs === undefined) {
    t.fail();
  } else {
    t.is(functionArgs.buildInfoDir, 'build-info.json');
    t.is(functionArgs.opts.unsafeAllowRenames, false);
    t.is(functionArgs.opts.unsafeSkipStorageCheck, false);
    t.is(functionArgs.opts.unsafeAllowCustomTypes, false);
    t.is(functionArgs.opts.unsafeAllowLinkedLibraries, false);
    t.deepEqual(functionArgs.opts.unsafeAllow, []);
  }
});

test('withDefaults - empty', t => {
  const parsedArgs = minimist(['validate', 'build-info.json']);
  const opts = withDefaults(parsedArgs);
  t.is(opts.unsafeAllowRenames, false);
  t.is(opts.unsafeSkipStorageCheck, false);
  t.is(opts.unsafeAllowCustomTypes, false);
  t.is(opts.unsafeAllowLinkedLibraries, false);
  t.deepEqual(opts.unsafeAllow, []);
});

test('withDefaults - some', t => {
  const parsedArgs = minimist([
    'validate',
    'build-info.json',
    '--unsafeAllowRenames',
    '--unsafeAllow',
    'selfdestruct, delegatecall,constructor',
  ]);
  const opts = withDefaults(parsedArgs);
  t.is(opts.unsafeAllowRenames, true);
  t.is(opts.unsafeSkipStorageCheck, false);
  t.is(opts.unsafeAllowCustomTypes, false);
  t.is(opts.unsafeAllowLinkedLibraries, false);
  t.deepEqual(opts.unsafeAllow, ['selfdestruct', 'delegatecall', 'constructor']);
});

test('withDefaults - all', t => {
  const parsedArgs = minimist([
    'validate',
    'build-info.json',
    '--unsafeAllowRenames',
    '--unsafeSkipStorageCheck',
    '--unsafeAllowCustomTypes',
    '--unsafeAllowLinkedLibraries',
    '--unsafeAllow',
    ...errorKinds,
  ]);
  const opts = withDefaults(parsedArgs);
  t.is(opts.unsafeAllowRenames, true);
  t.is(opts.unsafeSkipStorageCheck, true);
  t.is(opts.unsafeAllowCustomTypes, true);
  t.is(opts.unsafeAllowLinkedLibraries, true);
  t.true(opts.unsafeAllow.every((kind: string) => errorKinds.includes(kind as (typeof errorKinds)[number])));
});

test('withDefaults - invalid unsafeAllow', t => {
  const parsedArgs = minimist(['validate', 'build-info.json', '--unsafeAllow', 'foo']);
  t.throws(() => withDefaults(parsedArgs), {
    message: `Invalid option: --unsafeAllow "foo". Supported values for the --unsafeAllow option are: ${errorKinds.join(
      ', ',
    )}`,
  });
});
