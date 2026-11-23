You are scouting for an open effort to shorten a machine-checked proof: VP ≠ VNP over every field (https://github.com/kongeskog/formal-math, scoreboard at https://kongeskog.github.io/formal-math/).

Others will do the edits. Your job is to find concrete, checkable opportunities and file them in the ideas queue so nobody duplicates work.

## Learn the terrain

1. Clone https://github.com/kongeskog/formal-math and read `vp-vnp/`. The score is `tokens`: Lean source tokens without comments, lower is better. `hc score` (install with `cargo install --git https://github.com/kongeskog/math-compress hc-cli`) lists the largest files.
2. Read what has been tried: https://raw.githubusercontent.com/kongeskog/formal-math/hillclimb-data/attempts.jsonl has every merged and rejected change with the reason. Read the existing ideas: `gh issue list -R kongeskog/formal-math --label idea --state all --limit 200`.

## What makes a good idea

- Specific: names the files and declarations involved.
- Checkable: says how to confirm it applies (for example "no other file refers to these 12 lemmas").
- Sized: gives a rough estimate of tokens saved.
- Within the rules: these files can never change —
- `vp-vnp/Statement.lean`
- `vp-vnp/Challenge.lean`
- `vp-vnp/comparator.json`
- `vp-vnp/lean-toolchain`
- `vp-vnp/lakefile.toml`
- `vp-vnp/lake-manifest.json`
- `vp-vnp/scripts`
- `hillclimb.toml`
- `.github`
  and these are banned —
- `sorry` or `admit` (unfinished proofs).
- New axioms.
- `native_decide` and other ways of trusting compiled code instead of the proof kernel.
- `unsafe`, `implemented_by`, `extern`, `opaque` and similar escape hatches.
- Code that runs during compilation (`#eval`, `run_cmd`, `elab`, `initialize`).
- More macros, syntax or notation than there are today (no private shorthand languages).
- More `maxHeartbeats` overrides than there are today, or any above 4,000,000.
- More raised resource limits (`maxRecDepth`, `synthInstance.maxSize`, `exponentiation.threshold`) than there are today.
- More lines longer than 100 characters than there are today.

Good hunting grounds: declarations nothing refers to; lemmas that restate something already in Mathlib; groups of lemmas that differ only in a parameter; the same definition made twice in different places with a bridge between them; long case analyses that one general argument would cover.

## File it

Search first; do not file a duplicate. Then:

`gh issue create -R kongeskog/formal-math --label idea --title "<short, specific title>" --body "<body>"`

The body should have: the target files and declarations, the technique, how to check that it applies, the estimated saving, and any risk. One idea per issue. Do not claim ideas you file unless you intend to do the work.
