import test from 'ava';

import { getAnnotationArgs } from './run';

test('getAnnotationArgs', t => {
  const doc = ' @custom:oz-upgrades-unsafe-allow constructor selfdestruct';
  t.deepEqual(getAnnotationArgs(doc, 'oz-upgrades-unsafe-allow'), ['constructor', 'selfdestruct']);
});

test('getAnnotationArgs no arg', t => {
  const doc = ' @custom:oz-upgrades-unsafe-allow';
  t.deepEqual(getAnnotationArgs(doc, 'oz-upgrades-unsafe-allow'), []);
});

test('getAnnotationArgs space no arg', t => {
  const doc = ' @custom:oz-upgrades-unsafe-allow ';
  t.deepEqual(getAnnotationArgs(doc, 'oz-upgrades-unsafe-allow'), []);
});

test('getAnnotationArgs whitespace at end', t => {
  const doc = ' @custom:oz-upgrades-unsafe-allow constructor ';
  t.deepEqual(getAnnotationArgs(doc, 'oz-upgrades-unsafe-allow'), ['constructor']);
});

test('getAnnotationArgs same type', t => {
  const doc = ' @custom:oz-upgrades-unsafe-allow constructor\n @custom:oz-upgrades-unsafe-allow selfdestruct';
  t.deepEqual(getAnnotationArgs(doc, 'oz-upgrades-unsafe-allow'), ['constructor', 'selfdestruct']);
});

test('getAnnotationArgs multiline', t => {
  const doc =
    ' othercomments\n @custom:oz-upgrades-unsafe-allow constructor selfdestruct\n @custom:oz-upgrades-unsafe-allow-reachable delegatecall';
  t.deepEqual(getAnnotationArgs(doc, 'oz-upgrades-unsafe-allow'), ['constructor', 'selfdestruct']);
  t.deepEqual(getAnnotationArgs(doc, 'oz-upgrades-unsafe-allow-reachable'), ['delegatecall']);
});

test('getAnnotationArgs multiline with spaces and comments', t => {
  const doc =
    ' some other comments\n @custom:oz-upgrades-unsafe-allow \n   constructor    \n   selfdestruct    \n @custom:oz-upgrades-unsafe-allow-reachable  \n   delegatecall';
  t.deepEqual(getAnnotationArgs(doc, 'oz-upgrades-unsafe-allow'), ['constructor', 'selfdestruct']);
  t.deepEqual(getAnnotationArgs(doc, 'oz-upgrades-unsafe-allow-reachable'), ['delegatecall']);
});

test('getAnnotationArgs multiline multiple preceding spaces', t => {
  const doc =
    ' @custom:oz-upgrades-unsafe-allow constructor selfdestruct \n         @custom:oz-upgrades-unsafe-allow-reachable delegatecall    ';
  t.deepEqual(getAnnotationArgs(doc, 'oz-upgrades-unsafe-allow'), ['constructor', 'selfdestruct']);
  t.deepEqual(getAnnotationArgs(doc, 'oz-upgrades-unsafe-allow-reachable'), ['delegatecall']);
});
