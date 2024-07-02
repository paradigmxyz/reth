import { Context, SubResolver } from "@resolver-engine/core";
import Debug from "debug";
import GitInfo from "hosted-git-info";

const debug = Debug("resolverengine:githubresolver");

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
export function GithubResolver(): SubResolver {
  return async function github(what: string, ctx: Context): Promise<string | null> {
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
      const gitInfo = GitInfo.fromUrl(url + (comittish || ""));
      if (!gitInfo) {
        return null;
      }
      const fileUrl = gitInfo.file(file);
      debug("Resolved uri to:", fileUrl);
      return fileUrl;
    }

    return null;
  };
}
