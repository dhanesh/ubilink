<!-- R6 (human checkpoint). Keep diffs small and reversible; the reviewer is the
     gate that CI can't be. Delete sections that don't apply. -->

## What & why
<!-- One or two sentences: what changes, and the reason. Link the constraint /
     decision if relevant (e.g. DECISIONS.md D#, a .manifold/ constraint id). -->

## How verified
- [ ] `make verify` passes locally (fmt + clippy + all crate tests + JS + spec)
- [ ] CI is green on this PR
- [ ] Manual check, if behavior is user-visible (what you ran, what you saw)

## Reviewability & rollback
- [ ] Diff is scoped to one concern (mechanical churn split from logic)
- [ ] Change is reversible with a single `git revert` (no irreversible migration/side effect) — or the irreversible step is called out below

## Risk / blast radius
<!-- What could this break? Which crate(s)? Anything touching the capability
     broker, sandbox, signing, or the link graph deserves extra eyes. -->
