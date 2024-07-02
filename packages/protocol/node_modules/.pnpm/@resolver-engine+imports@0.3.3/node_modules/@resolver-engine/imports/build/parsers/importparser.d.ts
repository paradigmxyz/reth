import { SubParser } from "@resolver-engine/core";
export interface ImportFile {
    url: string;
    source: string;
    provider: string;
}
export declare function ImportParser(sourceParsers: Array<SubParser<string>>): SubParser<ImportFile>;
