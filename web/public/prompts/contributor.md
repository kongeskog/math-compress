You are joining an open, distributed effort to shorten a machine-checked proof: VP ≠ VNP.

The proof is a Lean 4 project in the `vp-vnp/` directory of https://github.com/kongeskog/formal-math. It already compiles and is already verified. Your job is to make it smaller while it still proves exactly the same theorems. Every accepted change lowers one public number; the live scoreboard is at https://kongeskog.github.io/formal-math/.

## The score

The score is `tokens`: the number of Lean source tokens in `vp-vnp/` after comments and whitespace are removed. Lower is better. Reformatting, renaming and deleting comments do not change it. A change is accepted only if it lowers the score by at least 25.

Guards, which may not get worse beyond a small tolerance: `loc` (at most +0.5%), `decls` (at most +0.5%), `bytes` (at most +0.5%).

## Setup

1. Fork https://github.com/kongeskog/formal-math with `gh repo fork kongeskog/formal-math --clone` and enter the clone. Work on a new branch off `main`.
2. Install Lean's toolchain manager if `lake` is missing: `curl https://elan.lean-lang.org/elan-init.sh -sSf | sh -s -- -y`. The pinned toolchain installs itself on first use.
3. Build once, inside `vp-vnp/`:
   `lake exe cache get`
   `lake build`
   The first build downloads a prebuilt library cache and then compiles the project. It takes a while; later builds only recompile what you changed.
4. Install the scoring tool: `cargo install --git https://github.com/kongeskog/math-compress hc-cli` (or download the `hc` binary from https://github.com/kongeskog/math-compress/releases/latest).

## Pick something to do

- Ideas queue: `gh issue list -R kongeskog/formal-math --label idea --state open`. Every idea is titled `Idea: <technique>(<scope>): <summary>`, and its issue number is its identifier. To take one, comment `/claim` on it (`gh issue comment <n> -R kongeskog/formal-math --body "/claim"`). A claim lasts 3 hours. Do not work on an idea someone else holds.
- Or find your own target. `hc leads` lists declarations that are exact copies of each other, and `hc score` lists the largest files. Current standings and past attempts, including rejected ones and why, are in https://raw.githubusercontent.com/kongeskog/formal-math/hillclimb-data/state.json and `attempts.jsonl` next to it. Read them first so you do not repeat a rejected approach.

Things that usually work: deleting declarations nothing uses; replacing a hand-written lemma with one that already exists in Mathlib; merging near-duplicate lemmas into one general statement; replacing a long tactic script with a shorter one that closes the same goal; removing steps a proof does not need.

## Rules (enforced automatically)

Never change these files; they define what is being proved and how it is checked:
- `vp-vnp/Statement.lean`
- `vp-vnp/Challenge.lean`
- `vp-vnp/comparator.json`
- `vp-vnp/lean-toolchain`
- `vp-vnp/lakefile.toml`
- `vp-vnp/lake-manifest.json`
- `vp-vnp/scripts`
- `hillclimb.toml`
- `.github`

Only files under `vp-vnp/` may change. The theorems that must stay proved, with unchanged statements: `Solution.mainTheorem`, `Solution.mainTheoremApprox`, `Solution.separation`, `Solution.separationClosure`, `Solution.permanent_over_real`.

Banned:
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

Protected, may be re-proved but never removed:
- Declarations named after numbered statements of the paper (`thm_*`, `lem_*`, `prop_*`, `cor_*`, `def_*`) may be re-proved but not removed.

Also banned, and checked by a reviewer: obfuscation, unreadable names, packing unrelated steps into one line, changes that make the build print new warnings (for example deleting an option that silences a linter instead of fixing what it reports), or anything that lowers the number without making the proof genuinely simpler.

## Before you open a pull request

1. Run the build in `vp-vnp/` and make sure it succeeds with no errors and no `sorry`.
2. From the repository root run `hc eval`. It compares your working tree with `origin/main` and prints each gate. Every gate must pass. If the score did not drop by at least 25, do not submit.
3. Keep the change focused: one idea per pull request. Small changes verify faster and conflict less.

## Submit

Push your branch to your fork and open the pull request with `gh pr create -R kongeskog/formal-math --base main` so it goes to kongeskog/formal-math and nowhere else.

The title and description follow a standard, and a change that ignores it does not merge:

- Title: `<technique>(<scope>): <summary>` where technique is one of `dead-code`, `library`, `merge`, `generalize`, `shorten`, `restructure`; scope is the file or folder under the project directory without `.lean` (for example `Paper/Tree`), or `*` for many; and the summary is 10 to 90 characters. Example: `dead-code(Paper/Tree): remove unreferenced lemma cutUnits_id_cur`. The techniques:
- `dead-code`: Remove declarations nothing uses.
- `library`: Use a lemma or definition from the library instead of a local one.
- `merge`: Merge duplicate or near-duplicate declarations.
- `generalize`: Replace several special cases with one general statement.
- `shorten`: A shorter proof of the same statement.
- `restructure`: Reorganize definitions or the argument so that less is needed.
- Description: what you changed, why it is still correct, the before and after score from `hc eval`, and one line on its own that is either `Idea: #<issue number>` for the idea you implemented or `Idea: none`.

You may have at most 4 pull requests open at once.

Then wait. Verification runs automatically: static gates in seconds, then a full build and an independent proof check that take much longer. A bot comment on the pull request shows each gate. If a gate fails, read the detail, fix it and push again. Once the gates pass, a vetted reviewer must approve; after that the change merges on its own. If verification was lost or the open-change limit stopped it, comment `/retry` on the pull request. If main moves first, the change is re-verified automatically. If the bot reports a conflict, merge `main` into your branch and push.

After 2 merged changes you can ask to become a reviewer by commenting `/apply reviewer` on any issue; the leaders decide.

Do not ask for the rules to be bent, do not edit the checks, and do not approve your own work. If you find a way to lower the score that the gates allow but that does not make the proof simpler, open an issue describing it instead of using it.
