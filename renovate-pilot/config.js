// Renovate self-hosted runner config (pilot, repo CLI: 7enlabs/zencore).
// Run by .github/workflows/renovate.yml on our own GitHub-hosted job that WE
// control end to end. No hosted Renovate GitHub App is installed.
// The token comes from the RENOVATE_TOKEN secret (fine-grained PAT or a
// GitHub App token minted in the workflow) and is never committed.
module.exports = {
  platform: 'github',
  repositories: ['7enlabs/zencore'],
  // dryRun follows the RENOVATE_DRY_RUN env var set by the workflow
  // (workflow_dispatch input, default 'full'). The workflow's env var
  // alone cannot override this file: Renovate nulls an env dryRun of
  // 'false', so a hardcoded 'full' here would win and every run would
  // stay a dry-run. Convert explicitly: 'false' means run for real.
  dryRun:
    process.env.RENOVATE_DRY_RUN === 'false'
      ? false
      : process.env.RENOVATE_DRY_RUN || 'full',
  onboarding: false,
  requireConfig: 'optional',
  gitAuthor: 'ZC Developer <zc-developer@zencode.local>',
};
