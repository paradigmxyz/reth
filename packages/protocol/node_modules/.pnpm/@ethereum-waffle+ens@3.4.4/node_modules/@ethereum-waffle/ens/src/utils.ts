import {ContractFactory, Signer, utils} from 'ethers';
import {ExpectedTopLevelDomain, InvalidDomain} from './errors';

const {namehash} = utils;

export const COIN_TYPE_ETH = 60;

export const deployContract = async (signer: Signer, contractJSON: any, args: Array<any>) => {
  const factory = new ContractFactory(contractJSON.abi, contractJSON.bytecode, signer);
  return factory.deploy(...args);
};

interface ENSDomainInfo {
  chunks: string [];
  tld: string;
  rawLabel: string;
  label: string;
  node: string;
  rootNode: string;
  decodedRootNode: string;
}

export const getDomainInfo = (domain: string): ENSDomainInfo => {
  const chunks = domain.split('.');
  const isTopLevelDomain = (chunks.length === 1 && chunks[0].length > 0);
  const isEmptyDomain = (domain === '');

  if (isTopLevelDomain) {
    throw new ExpectedTopLevelDomain();
  } else if (isEmptyDomain) {
    throw new InvalidDomain(domain);
  }
  try {
    namehash(domain);
  } catch (e) {
    throw new InvalidDomain(domain);
  }

  return {
    chunks,
    tld: chunks[chunks.length - 1],
    rawLabel: chunks[0],
    label: utils.id(chunks[0]),
    node: namehash(domain),
    rootNode: namehash(domain.replace(chunks[0] + '.', '')),
    decodedRootNode: domain.replace(chunks[0] + '.', '')
  };
};
