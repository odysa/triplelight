# Agent and contributor rules

## Code structure: deep modules, narrow interfaces (task #107)

- Run [`deslop-please`](https://github.com/deterministic-machines/deslop-please) to check for sloppy code structure:
  `ci/deslop.sh` (deslop-please 0.1.3, with the `rust-analyzer` component). CI runs it on every change.
- Errors flagged by `deslop-please` must be fixed from first principles. Don't make mechanical moves to hide them.
  - `module-cycle` & `tangle`: what is the intended layer of abstraction? Which module belongs at which layer?
  - `field-leak` & `shallow-module`: why do these fields or functions exist at all? What are their patterns of use?
    Can they be hidden behind a narrow interface? Don't fix each leak one at a time; fix them holistically.
  - Draw a diagram in the PR showing the structure before and after.
- `.deslop-please.allow` lists the existing findings (the baseline in `docs/deslop-baseline-v1/`). It only shrinks:
  a PR that fixes a finding deletes its line, and CI fails on a new finding or on a stale line. Never add a line to
  hide a new finding.
