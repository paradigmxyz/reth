/**
 * Class representing a CID `<mbase><version><mcodec><mhash>`
 * , as defined in [ipld/cid](https://github.com/multiformats/cid).
 */
declare class CID {
  /**
   * Create a new CID.
   *
   * The algorithm for argument input is roughly:
   * ```
   * if (cid)
   *   -> create a copy
   * else if (str)
   *   if (1st char is on multibase table) -> CID String
   *   else -> bs58 encoded multihash
   * else if (Buffer)
   *   if (1st byte is 0 or 1) -> CID
   *   else -> multihash
   * else if (Number)
   *   -> construct CID by parts
   * ```
   *
   * @example
   * new CID(<version>, <codec>, <multihash>, <multibaseName>)
   * new CID(<cidStr>)
   * new CID(<cid.buffer>)
   * new CID(<multihash>)
   * new CID(<bs58 encoded multihash>)
   * new CID(<cid>)
   */
  constructor(
    version: 0 | 1,
    codec: string,
    multhash: Buffer,
    multibaseName?: string
  );
  constructor(cid: CID);
  constructor(str: string);
  constructor(buf: Buffer);

  /**
   * The version of the CID.
   */
  version: number;

  /**
   * The codec of the CID.
   */
  codec: string;

  /**
   * The multihash of the CID.
   */
  multihash: Buffer;

  /**
   * Multibase name as string.
   */
  multibaseName: string;

  /**
   * The CID as a `Buffer`
   */
  readonly buffer: Buffer;

  /**
   * The prefix of the CID.
   */
  readonly prefix: Buffer;

  /**
   * Convert to a CID of version `0`.
   */
  toV0(): CID;

  /**
   * Convert to a CID of version `1`.
   */
  toV1(): CID;

  /**
   * Encode the CID into a string.
   *
   * @param base Base encoding to use.
   */
  toBaseEncodedString(base?: string): string;

  /**
   * Encode the CID into a string.
   */
  toString(base?: string): string;

  /**
   * Serialize to a plain object.
   */
  toJSON(): { codec: string; version: 0 | 1; hash: Buffer };

  /**
   * Compare equality with another CID.
   *
   * @param other The other CID.
   */
  equals(other: any): boolean;

  /**
   * Test if the given input is a valid CID object.
   * Throws if it is not.
   *
   * @param other The other CID.
   */
  static validateCID(other: any): void;

  static isCID(mixed: any): boolean;

  static codecs: Record<string, number>;
}

export = CID
