// Filter function for labels we do not want on PRs automatically.
function shouldIncludeLabel (label) {
    const isStatus = label.startsWith('S-');
    const isTrackingIssue = label === 'C-tracking-issue';
    const isPreventStale = label === 'M-prevent-stale';
    const isDifficulty = label.startsWith('D-');

    return !isStatus && !isTrackingIssue && !isPreventStale && !isDifficulty;
}

module.exports = async ({ github, context }) => {
    try {
        const prNumber = context.payload.pull_request.number;
        const prBody = context.payload.pull_request.body;
        const repo = context.repo;

        const repoUrl = context.payload.repository.html_url;
        const pattern = new RegExp(`(close|closes|closed|fix|fixes|fixed|resolve|resolves|resolved) ${repoUrl}/issues/(?<issue_number>\\d+)`, 'i')

        const re = prBody.match(pattern);
        const issueNumber = re?.groups?.issue_number;

        if (!issueNumber) {
            console.log('No issue reference found in PR description.');
            return;
        }

        const issue = await github.rest.issues.get({
            ...repo,
            issue_number: issueNumber,
        });

        const issueLabels = issue.data.labels
            .map(label => label.name)
            .filter(shouldIncludeLabel);
        if (issueLabels.length > 0) {
            await github.rest.issues.addLabels({
                ...repo,
                issue_number: prNumber,
                labels: issueLabels,
            });
        }
    } catch (err) {
        console.error('Failed to label PR');
        console.error(err);
    }
}
