# WebSearch

Search the web and return titles, URLs and short snippets. Keyless backend
(DuckDuckGo HTML); no API key, no account.

## Parameters

- `query` (string, required): Search query.
- `count` (integer, optional): Max results to return (default 10, max 10).

## Behavior

- Returns at most 10 results; each result is a title, a URL and a snippet of
  at most about 500 characters.
- The search request times out after 10 seconds; only `https` is used.
- The target host is checked with the same SSRF guard as `WebFetch`
  (private / loopback / link-local addresses are refused).
- The result page is HTML, not a stable API: if the provider serves a
  bot-check page or the layout changes, the tool fails with a clear error
  instead of returning empty results.
- The tool does not fetch page content; use `WebFetch` for that.

## Permissions

Mutating tool (same group as `WebFetch`): asks for permission in `default`
mode; denied in `-p` when it cannot ask. See SPEC.md §4-5.

## Phase bundle

`read` (exploration).
