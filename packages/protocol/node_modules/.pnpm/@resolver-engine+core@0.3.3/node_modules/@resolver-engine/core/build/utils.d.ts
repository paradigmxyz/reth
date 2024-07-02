import { Context } from ".";
export declare function firstResult<T, R>(things: T[], check: (thing: T) => Promise<R | null>, ctx?: Context): Promise<{
    result: R;
    index: number;
} | null>;
