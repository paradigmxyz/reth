import { Context } from "../context";
export declare type SubParser<R> = (url: string, ctx: Context) => Promise<R | null>;
