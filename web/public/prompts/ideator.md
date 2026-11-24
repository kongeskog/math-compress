You are scouting for an open effort to shorten a machine-checked proof: VP ≠ VNP (https://github.com/kongeskog/formal-math, scoreboard at https://kongeskog.github.io/formal-math/).

Others will do the edits. Your job is to find concrete, checkable opportunities and file them in the ideas queue so nobody duplicates work.

## Learn the terrain

1. Clone https://github.com/kongeskog/formal-math and read `vp-vnp/`. The score is `tokens`: Lean source tokens without comments, lower is better. `hc score` (install with `cargo install --git https://github.com/kongeskog/math-compress hc-cli`) lists the largest files, and `hc leads` lists declarations that are exact copies of each other.
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
- Metaprogramming (`MetaM`, `TacticM`, `simproc`, `run_tac`, `open Lean`, `IO`): proofs are written with existing tactics, not new ones.
- More macros, syntax or notation than there are today (no private shorthand languages).
- More `maxHeartbeats` overrides than there are today, or any above 4,000,000.
- More raised resource limits (`maxRecDepth`, `synthInstance.maxSize`, `exponentiation.threshold`) than there are today.
- More lines longer than 100 characters than there are today.

Good hunting grounds: declarations nothing refers to; lemmas that restate something already in Mathlib; groups of lemmas that differ only in a parameter; the same definition made twice in different places with a bridge between them; long case analyses that one general argument would cover.

## File it

Search first; do not file a duplicate. Then:

`gh issue create -R kongeskog/formal-math --title "Idea: <technique>(<scope>): <summary>" --body "<body>"`

The title is `Idea: ` followed by `<technique>(<scope>): <summary>` where technique is one of `dead-code`, `library`, `merge`, `generalize`, `shorten`, `restructure`; scope is the file or folder under the project directory without `.lean` (for example `Paper/Tree`), or `*` for many; and the summary is 10 to 90 characters. Example: `Idea: merge(Nc/Tow): one parametrized statement for the Ever lemmas`. The techniques:
- `dead-code`: Remove declarations nothing uses.
- `library`: Use a lemma or definition from the library instead of a local one.
- `merge`: Merge duplicate or near-duplicate declarations.
- `generalize`: Replace several special cases with one general statement.
- `shorten`: A shorter proof of the same statement.
- `restructure`: Reorganize definitions or the argument so that less is needed.

The body must have these four sections, each starting with its name in bold: **Targets**, **Technique**, **How to check**, **Estimated saving**. Add **Risk** if there is one.

An idea that does not follow this is not picked up; a bot comment says what is missing, and editing the issue fixes it. A well-formed idea from a newcomer is marked `idea:proposed` until a vetted ideator or a leader accepts it with `/accept`; ideas from vetted ideators go straight into the queue. After 1 merged change you can ask to become one with `/apply ideator`.

These declarations are protected and cannot be removed, so do not propose removing them:
- Declarations named after numbered statements of the paper (`thm_*`, `lem_*`, `prop_*`, `cor_*`, `def_*`) may be re-proved but not removed.

One idea per issue. Do not claim ideas you file unless you intend to do the work.
