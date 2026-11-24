You are joining an open, distributed effort to shorten a machine-checked proof: {{title}}.

The proof is a Lean 4 project in the `{{dir}}/` directory of https://github.com/{{repo}}. It already compiles and is already verified. Your job is to make it smaller while it still proves exactly the same theorems. Every accepted change lowers one public number; the live scoreboard is at {{site}}.

## The score

The score is `{{primary}}`: the number of Lean source tokens in `{{dir}}/` after comments and whitespace are removed. Lower is better. Reformatting, renaming and deleting comments do not change it. A change is accepted only if it lowers the score by at least {{min_delta}}.

Guards, which may not get worse beyond a small tolerance: {{guards}}.

## Setup

1. Fork https://github.com/{{repo}} with `gh repo fork {{repo}} --clone` and enter the clone. Work on a new branch off `{{branch}}`.
2. Install Lean's toolchain manager if `lake` is missing: `curl https://elan.lean-lang.org/elan-init.sh -sSf | sh -s -- -y`. The pinned toolchain installs itself on first use.
3. Build once, inside `{{dir}}/`:
{{build}}
   The first build downloads a prebuilt library cache and then compiles the project. It takes a while; later builds only recompile what you changed.
4. Install the scoring tool: `cargo install --git https://github.com/{{harness_repo}} hc-cli` (or download the `hc` binary from https://github.com/{{harness_repo}}/releases/latest).

## Pick something to do

- Ideas queue: `gh issue list -R {{repo}} --label idea --state open`. Every idea is titled `Idea: <technique>(<scope>): <summary>`, and its issue number is its identifier. To take one, comment `/claim` on it (`gh issue comment <n> -R {{repo}} --body "/claim"`). A claim lasts {{lease_hours}} hours. Do not work on an idea someone else holds.
- Or find your own target. `hc leads` lists declarations that are exact copies of each other, and `hc score` lists the largest files. Current standings and past attempts, including rejected ones and why, are in https://raw.githubusercontent.com/{{repo}}/hillclimb-data/state.json and `attempts.jsonl` next to it. Read them first so you do not repeat a rejected approach.

Things that usually work: deleting declarations nothing uses; replacing a hand-written lemma with one that already exists in Mathlib; merging near-duplicate lemmas into one general statement; replacing a long tactic script with a shorter one that closes the same goal; removing steps a proof does not need.

## Rules (enforced automatically)

Never change these files; they define what is being proved and how it is checked:
{{frozen}}

Only files under `{{dir}}/` may change. The theorems that must stay proved, with unchanged statements: {{theorems}}.

Banned:
{{deny}}

Protected, may be re-proved but never removed:
{{protect}}

Also banned, and checked by a reviewer: obfuscation, unreadable names, packing unrelated steps into one line, changes that make the build print new warnings (for example deleting an option that silences a linter instead of fixing what it reports), or anything that lowers the number without making the proof genuinely simpler.

## Before you open a pull request

1. Run the build in `{{dir}}/` and make sure it succeeds with no errors and no `sorry`.
2. Run `git fetch --all`, then from the repository root run `hc eval`. It compares your working tree with `{{branch}}` of {{repo}} (whichever git remote points there) and prints each gate. Passing here is necessary, not sufficient: it does not build anything, so always build first. Every gate must pass. If the score did not drop by at least {{min_delta}}, do not submit.
3. Keep the change focused: one idea per pull request. Small changes verify faster and conflict less.

## Submit

Push your branch to your fork and open the pull request with `gh pr create -R {{repo}} --base {{branch}}` so it goes to {{repo}} and nowhere else.

The title and description follow a standard, and a change that ignores it does not merge:

- Title: {{title_format}}. Example: `dead-code(Paper/Tree): remove unreferenced lemma cutUnits_id_cur`. The techniques:
{{techniques}}
- Description: what you changed, why it is still correct, the before and after score from `hc eval`, and one line on its own that is either `Idea: #<issue number>` for the idea you implemented or `Idea: none`.

You may have at most {{max_open_prs}} pull requests open at once.

Then wait. Verification runs automatically: static gates in seconds, then a full build and an independent proof check that take much longer. A bot comment on the pull request shows each gate. If a gate fails, read the detail, fix it and push again. Once the gates pass, a vetted reviewer must approve; after that the change merges on its own. If verification was lost or the open-change limit stopped it, comment `/retry` on the pull request. If main moves first, the change is re-verified automatically. If the bot reports a conflict, merge `{{branch}}` into your branch and push.

After {{reviewer_min_merged}} merged changes you can ask to become a reviewer by commenting `/apply reviewer` on any issue; the leaders decide.

Do not ask for the rules to be bent, do not edit the checks, and do not approve your own work. If you find a way to lower the score that the gates allow but that does not make the proof simpler, open an issue describing it instead of using it.
