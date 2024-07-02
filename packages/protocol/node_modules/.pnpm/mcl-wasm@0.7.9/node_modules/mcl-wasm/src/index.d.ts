export enum CurveType {
    BN254 = 0,
    BN_SNARK1 = 4,
    BLS12_381 = 5,

    SECP224K1 = 101,
    SECP256K1 = 102,
    NIST_P192 = 105,
    NIST_P224 = 106,
    NIST_P256 = 107,
}

export const BN254 = CurveType.BN254;
export const BN_SNARK1 = CurveType.BN_SNARK1;
export const BLS12_381 = CurveType.BLS12_381;
export const SECP224K1 = CurveType.SECP224K1;
export const SECP256K1 = CurveType.SECP256K1;
export const NIST_P192 = CurveType.NIST_P192;
export const NIST_P224 = CurveType.NIST_P224;
export const NIST_P256 = CurveType.NIST_P256;
export const IRTF = 5;
export const EC_PROJ = 1024;

declare class Common {
    deserializeHexStr(s: string): void;
    serializeToHexStr(): string;
    dump(msg?: string): void;
    clear(): void;
    clone(): this;
    setStr(s: string, base?: number): void;
    getStr(base?: number): string;
    isEqual(rhs: this): boolean
    isZero(): boolean;
    deserialize(v: Uint8Array): void;
    serialize(): Uint8Array;
    setHashOf(a: string | Uint8Array): void;
}

declare class IntType extends Common {
    setInt(x: number): void;
    isOne(): boolean;
    setLittleEndian(a: Uint8Array): void;
    setLittleEndianMod(a: Uint8Array): void;
    setBigEndianMod(a: Uint8Array): void;
    setByCSPRNG(): void;
}

declare class Fp extends IntType {
    __Fp:never; // to distinct Fp and Fr
    mapToG1(): G1;
}

declare class Fr extends IntType {
    __Fr:never; // to distinct Fp and Fr
}

declare class Fp2 extends Common {
    setInt(x: number, y: number): void;
    get_a(): Fp;
    get_b(): Fp;
    set_a(v: Fp): void;
    set_b(v: Fp): void;
    mapToG2(): G2;
}

declare class EllipticType extends Common {
    normalize(): void;
    isValid(): boolean;
    isValidOrder(): boolean;
}

declare class G1 extends EllipticType {
    setX(v: Fp): void;
    setY(v: Fp): void;
    setZ(v: Fp): void;
    getX(): Fp;
    getY(): Fp;
    getZ(): Fp;
}

declare class G2 extends EllipticType {
    setX(v: Fp2): void;
    setY(v: Fp2): void;
    setZ(v: Fp2): void;
    getX(): Fp2;
    getY(): Fp2;
    getZ(): Fp2;
}

declare class GT extends Common {
    setInt(x: number): void;
    isOne(): boolean;
}

declare class PrecomputedG2 {
    constructor(Q: G2);
    destroy(): void;
}

export function init(curveType?: CurveType): Promise<void>;

export function deserializeHexStrToFr(s: string): Fr;
export function deserializeHexStrToFp(s: string): Fp;
export function deserializeHexStrToFp2(s: string): Fp2;
export function deserializeHexStrToG1(s: string): G1;
export function deserializeHexStrToG2(s: string): G2;
export function deserializeHexStrToGT(s: string): GT;

export function setETHserialization(b: boolean): void;
export function setMapToMode(mode: number): void;
export function verifyOrderG1(doVerify: boolean): void;
export function verifyOrderG2(doVerify: boolean): void;

export function getBasePointG1(): G1;

export function neg(x: Fr): Fr;
export function neg(x: Fp): Fp;
export function neg(x: Fp2): Fp2;
export function neg(x: G1): G1;
export function neg(x: G2): G2;
export function neg(x: GT): GT;

export function sqr(x: Fr): Fr;
export function sqr(x: Fp): Fp;
export function sqr(x: Fp2): Fp2;
export function sqr(x: GT): GT;

export function inv(x: Fr): Fr;
export function inv(x: Fp): Fp;
export function inv(x: Fp2): Fp2;
export function inv(x: GT): GT;

export function normalize(x: G1): G1;
export function normalize(x: G2): G2;

export function add(x: Fr, y: Fr): Fr;
export function add(x: Fp, y: Fp): Fp;
export function add(x: Fp2, y: Fp2): Fp2;
export function add(x: G1, y: G1): G1;
export function add(x: G2, y: G2): G2;
export function add(x: GT, y: GT): GT;

export function sub(x: Fr, y: Fr): Fr;
export function sub(x: Fp, y: Fp): Fp;
export function sub(x: Fp2, y: Fp2): Fp2;
export function sub(x: G1, y: G1): G1;
export function sub(x: G2, y: G2): G2;
export function sub(x: GT, y: GT): GT;

export function mul(x: Fr, y: Fr): Fr;
export function mul(x: Fp, y: Fp): Fp;
export function mul(x: Fp2, y: Fp2): Fp2;
export function mul(x: G1, y: Fr): G1;
export function mul(x: G2, y: Fr): G2;
export function mul(x: GT, y: GT): GT;

export function mulVec(xVec: G1[], yVec: Fr[]): G1;
export function mulVec(xVec: G2[], yVec: Fr[]): G2;

export function div(x: Fr, y: Fr): Fr;
export function div(x: Fp, y: Fp): Fp;
export function div(x: Fp2, y: Fp2): Fp2;
export function div(x: GT, y: GT): GT;

export function dbl(x: G1): G1;
export function dbl(x: G2): G2;

export function hashToFr(s: string | Uint8Array): Fr;
export function hashAndMapToG1(s: string | Uint8Array): G1;
export function hashAndMapToG2(s: string | Uint8Array): G2;

export function pow(x: GT, y: Fr): GT;
export function pairing(P: G1, Q: G2): GT;
export function millerLoop(P: G1, Q: G2): GT;
export function finalExp(x: GT): GT;

export function precomputedMillerLoop(P: G1, Qcoeff: PrecomputedG2): GT;
export function precomputedMillerLoop2(P: G1, Q1coeff: PrecomputedG2, P2: G1, Q2coeff: PrecomputedG2): GT;
export function precomputedMillerLoop2mixed(P: G1, Q1: G2, P2: G1, Q2coeff: PrecomputedG2): GT;
