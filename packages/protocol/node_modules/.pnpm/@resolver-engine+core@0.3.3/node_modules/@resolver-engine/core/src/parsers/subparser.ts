import { Context } from "../context";
export type SubParser<R> = (url: string, ctx: Context) => Promise<R | null>;
