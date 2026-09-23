# Social preview image brief

The repo currently has no uploaded social preview, so GitHub generates a
fallback from the avatar and repo name. That fallback is what renders in every
Slack, Discord, X, and LinkedIn unfurl of a scaffold link.

## Specification

| Property | Value |
|---|---|
| Dimensions | 1280 × 640 px (GitHub's 2:1 ratio) |
| Format | PNG |
| File size | Under 1 MB |
| Upload | Settings → General → Social preview → Edit |
| Safe area | Keep all text inside a 1100 × 500 centered box. Clients crop the edges. |

## Content

Three elements, nothing else.

1. **Wordmark**: `logos-scaffold`. Monospace, heavy weight, largest element on
   the canvas.
2. **Tagline**: `Build, run, and deploy Logos programs against a local
   execution zone.` One line if it fits at a legible size, two if not. This is
   the README tagline, trimmed. Keep them in sync.
3. **Proof**: a terminal fragment showing the inner loop. Four lines is enough:

   ```
   $ lgs run
   [3/5] Ensuring localnet...
   [5/5] Deploying programs...
   Sequencer: http://127.0.0.1:3040
   ```

   Use real CLI output. Do not invent step labels; they come from
   `src/commands/run.rs`.

## Design notes

- Unfurls render small. Test at 25% zoom: if the tagline is unreadable there,
  the type is too small. Wordmark should survive down to a 320 px wide preview.
- Dark background, light text. Most unfurl surfaces are dark, and a light card
  glares.
- One accent color, used once. Suggested: the `Sequencer:` line or the
  `[5/5]` step marker, so the eye lands on the payoff.
- No stock imagery, no gradients behind text, no logo soup. The terminal
  fragment is the visual.
- Leave the bottom-right corner clear. Some clients overlay a domain label
  there.

## Do not include

- Star counts, download numbers, or any metric. They date immediately and this
  project's numbers are small enough that quoting them works against it.
- "Powered by" badges or partner logos.
- A version number. It forces a re-render on every release.
