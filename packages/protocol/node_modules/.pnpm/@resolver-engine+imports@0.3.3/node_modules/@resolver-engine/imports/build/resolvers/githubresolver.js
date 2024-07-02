"use strict";
var __awaiter = (this && this.__awaiter) || function (thisArg, _arguments, P, generator) {
    return new (P || (P = Promise))(function (resolve, reject) {
        function fulfilled(value) { try { step(generator.next(value)); } catch (e) { reject(e); } }
        function rejected(value) { try { step(generator["throw"](value)); } catch (e) { reject(e); } }
        function step(result) { result.done ? resolve(result.value) : new P(function (resolve) { resolve(result.value); }).then(fulfilled, rejected); }
        step((generator = generator.apply(thisArg, _arguments || [])).next());
    });
};
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
const debug_1 = __importDefault(require("debug"));
const hosted_git_info_1 = __importDefault(require("hosted-git-info"));
const debug = debug_1.default("resolverengine:githubresolver");
// hosted-git-info is a godsend, but it doesn't support specific files
// 1st group - protocol, location, owner, repo
// 2nd group - file inside repo
// 3rd group - commitish
const GIT_HOSTED_INFO = /^((?:.+:\/\/)?[^:/]+[/:][^/]+[/][^/]+)[/](.+?)(#.+)?$/;
// 1. (owner), 2. (repo), 3. (commit/file)
const BROWSER_LINK = /^https?:\/\/github\.com\/([^/]+)\/([^/]+)\/blob\/((?:[^/]+[/])*[^/]+)$/;
// 1. (owner), 2. (repo), 3. (file); AFAIK no support for commits
const REMIX_GITHUB_LINK = /^https?:\/\/github\.com\/([^/]+)\/([^/]+)\/((?:[^/]+[/])*[^/]+)$/;
// TODO(ritave): Support private repositories
function GithubResolver() {
    return function github(what, ctx) {
        return __awaiter(this, void 0, void 0, function* () {
            const fileMatchLink = what.match(BROWSER_LINK);
            if (fileMatchLink) {
                const [, owner, repo, commitAndFile] = fileMatchLink;
                const gitRawUrl = `https://raw.githubusercontent.com/${owner}/${repo}/${commitAndFile}`;
                debug("Resolved uri to:", gitRawUrl);
                return gitRawUrl;
            }
            const fileMatchRemix = what.match(REMIX_GITHUB_LINK);
            if (fileMatchRemix) {
                const [, owner, repo, file] = fileMatchRemix;
                const gitRawUrl = `https://raw.githubusercontent.com/${owner}/${repo}/master/${file}`;
                debug("Resolved uri to:", gitRawUrl);
                return gitRawUrl;
            }
            const fileMatchGitHostedInfo = what.match(GIT_HOSTED_INFO);
            if (fileMatchGitHostedInfo) {
                const [, url, file, comittish] = fileMatchGitHostedInfo;
                const gitInfo = hosted_git_info_1.default.fromUrl(url + (comittish || ""));
                if (!gitInfo) {
                    return null;
                }
                const fileUrl = gitInfo.file(file);
                debug("Resolved uri to:", fileUrl);
                return fileUrl;
            }
            return null;
        });
    };
}
exports.GithubResolver = GithubResolver;
//# sourceMappingURL=githubresolver.js.map