# Package assets

The manifest references these files. All are still **missing** - they
come from the brand owner (Z Studio); do not fabricate replacements.

| File | Size | Used by |
| --- | --- | --- |
| `Square150x150Logo.png` | 150x150 | Start menu tile |
| `Square44x44Logo.png` | 44x44 | Taskbar, task view, app list |
| `StoreLogo.png` | 50x50 | Store listing thumbnail (`<Logo>` in `<Properties>`) |

Recommended on top (add before the first submission if available):
scaled variants `Square150x150Logo.scale-200.png` and friends, the
unplated taskbar variant
`Square44x44Logo.targetsize-44_altform-unplated.png`, and
`Wide310x150Logo.png` (310x150).

`makeappx pack` fails while a referenced asset is missing, so the
package build waits for these files. Placeholder images are fine on a
developer's own machine for a pipeline smoke test, but they must never
be committed here or submitted to the Store.
