import test from 'ava';

import { SolcInput, SolcOutput } from './solc-api';
import { solcInputOutputDecoder } from './src-decoder';

test('solcInputOutputDecoder', t => {
  const solcInput: SolcInput = {
    sources: {
      'a.sol': { content: '\n\n' },
      'b.sol': { content: '\n\n' },
    },
  };

  const solcOutput: SolcOutput = {
    contracts: {},
    sources: {
      'a.sol': { id: 0, ast: undefined as any },
      'b.sol': { id: 1, ast: undefined as any },
    },
  };

  const decodeSrc = solcInputOutputDecoder(solcInput, solcOutput);

  t.is(decodeSrc({ src: '0:0:0' }), 'a.sol:1');
  t.is(decodeSrc({ src: '1:0:0' }), 'a.sol:2');
  t.is(decodeSrc({ src: '1:0:1' }), 'b.sol:2');
  t.is(decodeSrc({ src: '2:0:1' }), 'b.sol:3');
});
