
const should = require('chai').should()
const expect = require('chai').expect
const contentHash = require('../src/index.js')

const ipfs = 'QmRAQB6YaCyidP37UdDnjFY5vQuiBrcqdyoW1CuDgwxkD4'
const ipfs_contentHash = 'e3010170122029f2d17be6139079dc48696d1f582a8530eb9805b561eda517e22a892c7e3f1f'
const ipns_contentHash = 'e5010170122029f2d17be6139079dc48696d1f582a8530eb9805b561eda517e22a892c7e3f1f'
const swarm = 'd1de9994b4d039f6548d191eb26786769f580809256b4685ef316805265ea162'
const swarm_contentHash = 'e40101fa011b20d1de9994b4d039f6548d191eb26786769f580809256b4685ef316805265ea162'
const onion = 'zqktlwi4fecvo6ri'
const onion_contentHash = 'bc037a716b746c776934666563766f367269';
const onion3 = 'p53lf57qovyuvwsc6xnrppyply3vtqm7l6pcobkmyqsiofyeznfu5uqd';
const onion3_contentHash = 'bd037035336c663537716f7679757677736336786e72707079706c79337674716d376c3670636f626b6d797173696f6679657a6e667535757164';
const ipfsBase32DagPb = 'bafybeibj6lixxzqtsb45ysdjnupvqkufgdvzqbnvmhw2kf7cfkesy7r7d4';
const ipfsBase32Libp2pKey = 'bafzbeie5745rpv2m6tjyuugywy4d5ewrqgqqhfnf445he3omzpjbx5xqxe';

describe('content-hash (legacy tests)', () =>
	{
		it('should decode a content hash', () => {
			const actual_0 = contentHash.decode(ipfs_contentHash);
			const actual_1 = contentHash.decode(swarm_contentHash);
			let actual_2 = contentHash.decode(onion_contentHash);

			actual_0.should.be.equal(ipfs);
			actual_1.should.be.equal(swarm);
			actual_2.should.be.equal(onion);
		});
		it('should encode an ipfs address', () => {
			const actual = contentHash.fromIpfs(ipfs);
			actual.should.be.equal(ipfs_contentHash);
		});
		it('should encode a swarm address', () => {
			const actual = contentHash.fromSwarm(swarm);
			actual.should.be.equal(swarm_contentHash);
		});
		it('should encode an onion address', () => {
			const actual = contentHash.encode('onion', onion);
			actual.should.be.equal(onion_contentHash);
		});
		it('should get a codec from a content hash', () => {
			const actual_0 = contentHash.getCodec(ipfs_contentHash);
			const actual_1 = contentHash.getCodec(swarm_contentHash);
			const actual_2 = contentHash.getCodec(onion_contentHash);

			actual_0.should.be.equal('ipfs-ns');
			actual_1.should.be.equal('swarm-ns');
			actual_2.should.be.equal('onion');
		});
	}
);

describe('content-hash', () => {
	describe('swarm', () => {
		it('should encode', () => {
			const actual = contentHash.encode('swarm-ns', swarm);
			actual.should.be.equal(swarm_contentHash);
		});
		it('should getCodec', () => {
			const actual = contentHash.getCodec(swarm_contentHash);
			actual.should.be.equal('swarm-ns');
		});
		it('should decode', () => {
			const actual = contentHash.decode(swarm_contentHash);
			actual.should.be.equal(swarm);
		});
	});
	describe('ipfs', () => {
		it('should encode', () => {
			const actual = contentHash.encode('ipfs-ns', ipfs);
			actual.should.be.equal(ipfs_contentHash);
		});
		it('should getCodec', () => {
			const actual = contentHash.getCodec(ipfs_contentHash);
			actual.should.be.equal('ipfs-ns');
		});
		it('should decode', () => {
			const actual = contentHash.decode(ipfs_contentHash);
			actual.should.be.equal(ipfs);
		});
	});
	describe('ipns', () => {
		it('should encode', () => {
			const actual = contentHash.encode('ipns-ns', ipfs); // ipns & ipfs are the same hash and same encoding, only codec differ
			actual.should.be.equal(ipns_contentHash);
		});
		it('should getCodec', () => {
			const actual = contentHash.getCodec(ipns_contentHash);
			actual.should.be.equal('ipns-ns');
		});
		it('should decode', () => {
			const actual = contentHash.decode(ipns_contentHash);
			actual.should.be.equal(ipfs); // ipns & ipfs are the same hash and same encoding, only codec differ
		});
	});
	describe('onion', () => {
		it('should encode', () => {
			const actual = contentHash.encode('onion', onion);
			actual.should.be.equal(onion_contentHash);
		});
		it('should getCodec', () => {
			const actual = contentHash.getCodec(onion_contentHash);
			actual.should.be.equal('onion');
		});
		it('should decode', () => {
			const actual = contentHash.decode(onion_contentHash);
			actual.should.be.equal(onion);
		});
	});
	describe('onion3', () => {
		it('should encode', () => {
			const actual = contentHash.encode('onion3', onion3);
			actual.should.be.equal(onion3_contentHash);
		});
		it('should getCodec', () => {
			const actual = contentHash.getCodec(onion3_contentHash);
			actual.should.be.equal('onion3');
		});
		it('should decode', () => {
			const actual = contentHash.decode(onion3_contentHash);
			actual.should.be.equal(onion3);
		});
	});
	describe('helpers', () => {
		it('should convert CID v0 into v1', () => {
			const actual = contentHash.helpers.cidV0ToV1Base32(ipfs);
			actual.should.be.equal(ipfsBase32DagPb);
		});
		it('should keep CID v1 Base32 as-is', () => {
			const dagPbCid = contentHash.helpers.cidV0ToV1Base32(ipfsBase32DagPb);
			dagPbCid.should.be.equal(ipfsBase32DagPb);
			const libp2pKeyCid = contentHash.helpers.cidV0ToV1Base32(ipfsBase32Libp2pKey);
			libp2pKeyCid.should.be.equal(ipfsBase32Libp2pKey);
		});
	});
});
