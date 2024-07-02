import chai, {expect} from 'chai';
import {constants, utils} from 'ethers';
import {getWallet} from './utils';
import {deployENS, ENS} from '../src/index';

import chaiAsPromised from 'chai-as-promised';

chai.use(chaiAsPromised);

const {AddressZero} = constants;
const {namehash} = utils;
const nonExistingNode = '0x0000000000000000000000000000000000000000000000000000000000000001';

describe('Deploy Ens', async () => {
  const wallet = getWallet();
  let ens: ENS;

  beforeEach(async () => {
    ens = await deployENS(wallet);
    await ens.createTopLevelDomain('test');
  });

  it('ENS deployed', async () => {
    expect(await ens.ens.owner(nonExistingNode)).to.equal(AddressZero);
  });

  it('PublicResolver deployed and setup', async () => {
    expect(await ens.ens.resolver(namehash('resolver'))).to.eq(ens.resolver.address);
    expect(await ens.ens.owner(namehash('resolver'))).to.eq(wallet.address);
    expect(await ens.resolver['addr(bytes32)'](namehash('resolver'))).to.eq(ens.resolver.address);
  });

  it('ReverseRegistrar deployed and setup', async () => {
    expect(await ens.ens.owner(namehash('reverse'))).to.eq(wallet.address);
    expect(await ens.ens.owner(namehash('addr.reverse'))).to.eq(ens.reverseRegistrar.address);
  });

  describe('Create domain', async () => {
    describe('Non recursive', async () => {
      it('top level domain', async () => {
        const node = namehash('tld');
        await ens.createTopLevelDomain('tld');
        expect(await ens.ens.owner(node)).to.eq(ens.registrars['tld'].address);
      });

      it('sub domain', async () => {
        const node = namehash('ethworks.test');
        await ens.createSubDomain('ethworks.test');
        expect(await ens.ens.owner(node)).to.eq(ens.registrars['ethworks.test'].address);
        expect(await ens.ens.resolver(node)).to.eq(ens.resolver.address);
      });

      it('third level domain', async () => {
        await ens.createSubDomain('ethworks.test');
        const node = namehash('dev.ethworks.test');
        await ens.createSubDomain('dev.ethworks.test');
        expect(await ens.ens.owner(node)).to.eq(ens.registrars['dev.ethworks.test'].address);
        expect(await ens.ens.resolver(node)).to.eq(ens.resolver.address);
      });
    });

    describe('Recursive', async () => {
      it('third level domain', async () => {
        const node = namehash('waffle.projects.tld');
        await ens.createSubDomain('waffle.projects.tld', {recursive: true});
        expect(await ens.ens.owner(node)).to.eq(ens.registrars['waffle.projects.tld'].address);
        expect(await ens.ens.resolver(node)).to.eq(ens.resolver.address);
      });
    });

    describe('Fail', async () => {
      it('third level domain for nonexistent second level domain', async () => {
        await expect(ens.createSubDomain('ens.nonexistent.test'))
          .to.be.rejectedWith('Domain nonexistent.test doesn\'t exist.');
      });
    });
  });

  describe('Set address', async () => {
    describe('Non recursive', async () => {
      it('existing domain', async () => {
        const node = namehash('vlad.ethworks.test');
        await ens.createSubDomain('ethworks.test');
        await ens.setAddress('vlad.ethworks.test', await ens.signer.getAddress());
        expect(await ens.ens.owner(node)).to.eq(await ens.signer.getAddress());
        expect(await ens.resolver['addr(bytes32)'](node)).to.eq(await ens.signer.getAddress());
        expect(await ens.ens.resolver(node)).to.eq(ens.resolver.address);
      });
    });

    describe('Reverse', async () => {
      it('reverse', async () => {
        await ens.setAddressWithReverse('vlad.ethworks.test', wallet, {recursive: true});
        const node = namehash((wallet.address).slice(2) + '.addr.reverse');
        expect(await ens.ens.owner(node)).to.eq(ens.reverseRegistrar.address);
        expect(await ens.ens.resolver(node)).to.eq(ens.resolver.address);
        expect(await ens.resolver.name(node)).to.eq('vlad.ethworks.test');
      });
    });

    describe('Recursive', async () => {
      it('nonexistent domain', async () => {
        const node = namehash('vlad.test.tld');
        await ens.setAddress('vlad.test.tld', await ens.signer.getAddress(), {recursive: true});
        expect(await ens.ens.owner(node)).to.eq(await ens.signer.getAddress());
        expect(await ens.resolver['addr(bytes32)'](node)).to.eq(await ens.signer.getAddress());
        expect(await ens.ens.resolver(node)).to.eq(ens.resolver.address);
      });
    });

    describe('Fail', async () => {
      it('nonexistent domain', async () => {
        await expect(ens.setAddress('vlad.nonexistent.test', await ens.signer.getAddress()))
          .to.be.rejectedWith('Domain nonexistent.test doesn\'t exist.');
      });
    });
  });
});
