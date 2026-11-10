// Renovate self-hosted runner config (pilot, repo CLI: 7enlabs/zencode).
// Run by .github/workflows/renovate.yml on our own GitHub-hosted job that WE
// control end to end. No hosted Renovate GitHub App is installed.
// The token comes from the RENOVATE_TOKEN secret (fine-grained PAT or a
// GitHub App token minted in the workflow) and is never committed.
module.exports = {
  platform: 'github',
  repositories: ['7enlabs/zencode'],
  // Pilot phase: lookups and branch/PR proposals are computed, and the
  // Dependency Dashboard issue shows what would be opened. Flip to
  // 'full' only after the Leader approves the pilot report.
  dryRun: 'full',
  onboarding: false,
  requireConfig: 'optional',
  gitAuthor: 'ZC Developer <zc-developer@zencode.local>',
};
