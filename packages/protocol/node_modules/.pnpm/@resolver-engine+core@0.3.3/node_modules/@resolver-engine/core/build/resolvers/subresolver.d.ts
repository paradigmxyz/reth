import { Context } from "../context";
export declare type SubResolver = (what: string, context: Context) => Promise<string | null>;
