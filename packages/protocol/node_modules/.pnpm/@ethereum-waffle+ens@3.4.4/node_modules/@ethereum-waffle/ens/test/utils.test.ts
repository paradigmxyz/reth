import {expect} from 'chai';
import {utils} from 'ethers';
import {getDomainInfo} from '../src/utils';

describe('UNIT: getDomainInfo', async () => {
  it('third level domain', () => {
    expect(getDomainInfo('vlad.ethworks.test'))
      .to.deep.include({
        chunks: ['vlad', 'ethworks', 'test'],
        tld: 'test',
        rawLabel: 'vlad',
        label: utils.id('vlad'),
        node: utils.namehash('vlad.ethworks.test'),
        rootNode: utils.namehash('ethworks.test'),
        decodedRootNode: 'ethworks.test'
      });
  });

  it('second level domain', () => {
    expect(getDomainInfo('ethworks.test'))
      .to.deep.include({
        chunks: ['ethworks', 'test'],
        tld: 'test',
        rawLabel: 'ethworks',
        label: utils.id('ethworks'),
        node: utils.namehash('ethworks.test'),
        rootNode: utils.namehash('test'),
        decodedRootNode: 'test'
      });
  });

  it('top level domain', () => {
    expect(() => getDomainInfo('test'))
      .to.throw('Invalid domain. Please, enter no top level domain.');
  });

  it('empty domain', () => {
    expect(() => getDomainInfo(''))
      .to.throw('Invalid domain: \'\'');
  });

  it('invalid domain', () => {
    expect(() => getDomainInfo('vlad..ethworks.test'))
      .to.throw('Invalid domain: \'vlad..ethworks.test\'');
  });
});
