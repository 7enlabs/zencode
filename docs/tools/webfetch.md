# WebFetch

Fetch a URL over HTTP(S) and return it as readable text.

## Parameters

- `url` (string, required): Absolute http(s) URL to fetch.

## Behavior

- Redirects are followed; requests time out after 10 seconds; responses are
  capped at 5 MB.
- HTML is converted to readable text (scripts and styles stripped).
- Mutating tool (same group as `Write`/`Bash`): asks for permission in
  `default` mode; denied in `-p` when it cannot ask. See SPEC.md §4-5.

## Phase bundle

`read` (exploration).
