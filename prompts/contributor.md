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

An idea can reserve the files it will rewrite while it is claimed. `hc eval --reservations` lists the current reservations and warns if your change touches one; the bot's comment on your pull request warns too. It is a warning, not a gate: coordinate on that idea's issue, or expect to merge `{{branch}}` into your branch after its change lands.

Things that usually work, largest first: deleting whatever the final theorems do not need, including fields, conjuncts and hypotheses that nothing uses (each takes its proof with it); proving one result through another where the project develops the same thing twice; replacing a hand-written lemma with one that already exists in Mathlib; merging near-duplicate lemmas into one general statement; replacing a long tactic script with a shorter one that closes the same goal; removing steps a proof does not need.

## Rules (enforced automatically)

Never change these files; they define what is being proved and how it is checked:
{{frozen}}

Only files under `{{dir}}/` may change. The theorems that must stay proved, with unchanged statements: {{theorems}}.

Banned:
{{deny}}

{{protect_contributor}}

{{#review_all}}Also banned, and checked by a reviewer:{{/review_all}}{{#review_large_only}}Also banned. A reviewer checks these on large changes only; on the others no gate can check them and nobody reads the change before it merges, so they rest on you:{{/review_large_only}}{{^review}}Also banned. No gate can check these, and in this arena nobody reads a change before it merges, so they rest on you (a leader who sees one can stop the change, or have it repaired after it merged):{{/review}} obfuscation, unreadable names, packing unrelated steps into one line, changes that make the build print new warnings (for example deleting an option that silences a linter instead of fixing what it reports), or anything that lowers the number without making the proof genuinely simpler.{{conduct_contributor}}

## Before you open a pull request

1. Run the build in `{{dir}}/` and make sure it succeeds with no errors and no `sorry`.
2. Run `git fetch --all`, then from the repository root run `hc eval`. It compares your working tree with `{{branch}}` of {{repo}} (whichever git remote points there) and prints each gate. Passing here is necessary, not sufficient: it does not build anything, so always build first. Every gate must pass. If the score did not drop by at least {{min_delta}}, do not submit.
3. Keep the change focused: one idea per pull request. Small changes verify faster and conflict less.
4. Run `hc precheck` from the repository root (with `--title "<title>" --body-file <file>` once you have written the description). It runs `hc eval`, so every static gate, and shows place by place what they found: a deleted declaration still named in a comment or module doc anywhere in `{{dir}}/`, and added lines over the length limit or ending in whitespace. It also warns about things no gate fails on: a deleted name that still occurs in code, lines broken in the middle of a product, counts of files in the description that do not match the diff, and open pull requests that change the same files. Fix every error it reports and look at every warning.
5. Then read your own diff once{{#review}} as a reviewer would. Most requests for changes are about these, and each costs a review round:{{/review}}{{^review}}, line by line. Nobody else reads it before it merges, and no machine checks these:{{/review}}
   - a comment or doc comment at a place you changed describes the proof or statement as it was before, or names a hypothesis that is no longer in scope there;
   - a hypothesis kept a name that described the old statement;
   - the description says something the diff does not do, or its other counts (sites, lemmas) do not match the diff;
   - a line you added is broken between a function and its argument;
   - a proof became harder to follow than the one it replaces.
6. If you replaced explicit steps by automation, or inlined a lemma into a long proof, time the changed files with `hc time --base <a built checkout of {{branch}}>` (build both checkouts first) and paste the Markdown table it prints into the description. Automation over nonlinear arithmetic is the usual cause of a slow file; so are extra hypotheses left in scope, which slow every later step: clear them.
7. Run `hc precheck` once more after your last edit, with `--title` and `--body-file`: it also checks the title format and line length.

## Submit

Push your branch to your fork and open the pull request with `gh pr create -R {{repo}} --base {{branch}}` so it goes to {{repo}} and nowhere else.

The title and description follow a standard, and a change that ignores it does not merge:

- Title: {{title_format}}. Example: `dead-code(Paper/Tree): remove unreferenced lemma cutUnits_id_cur`. The techniques:
{{techniques}}
- Description: what you changed, why it is still correct, the before and after score from `hc eval`, and one line on its own naming the idea: `Idea: #<issue number>` for the idea this change was made for (the idea is closed when it merges), or `Idea: none`. An idea is one piece of work: if your change leaves part of it undone, say what is left in the description so it can be filed as a new idea; only where the idea was written as a sweep to be done in several changes, write `Idea: #<issue number> (partial)`, which keeps it open.

You may have at most {{max_open_prs}} pull requests open at once.

Building on a change that has not merged yet: if your change needs another open pull request of {{repo}} (a follow-up to a large change, for example), do not wait for it to merge. Branch from that pull request's head, and put a line `Depends: #<its number>` in your description. Your change is then measured{{#review}}, verified and reviewed{{/review}}{{^review}} and verified{{/review}} as the difference from that pull request (in the steps above, run `hc eval` and `hc precheck` with `--depends-ref <its branch or commit>` to see the same numbers locally), and it merges right after it. Your branch must contain that pull request's head commit. If that pull request is revised, the bot's comment says so: merge its branch into yours and push. If it merged as another commit than the one your branch has, merge `{{branch}}` into your branch and push. If it has merged and the bot reports a conflict, merge `{{branch}}` into your branch and push. If it is closed instead, rebase onto `{{branch}}`, remove the line and push. The line belongs to the commit: changing or removing it needs a new push{{#review}}, and a push needs a new review{{/review}}. Use it only for a real dependency; changes that do not need each other merge faster apart.

Then wait. Verification runs automatically: static gates in seconds, then a full build and an independent proof check that take much longer. A bot comment on the pull request shows each gate. If a gate fails, read the detail, fix it and push again. {{#review_all}}Once the gates pass, a vetted reviewer must approve; after that the change merges on its own.{{/review_all}}{{#review_large_only}}Once every gate passes, a change of at most {{large_change_files}} files merges on its own: nobody has to approve it. Only a larger one needs a reviewer: {{review_large}} approval(s) from vetted reviewers, where the files of an unmerged change it is built on count with its own.{{/review_large_only}}{{^review}}Once every gate passes, the change merges on its own: nobody has to approve it.{{/review}} If verification was lost or the open-change limit stopped it, comment `/retry` on the pull request. If main moves first, the change is re-verified automatically. If the bot reports a conflict, merge `{{branch}}` into your branch and push.

{{^review}}
What the gates check, and all that is checked before a change merges:
{{gates}}

A leader can stop a change that passes them with a comment that has a line starting with `/request-changes`; the bot says so on the pull request, and the change then waits until that leader approves the commit. What nobody checks before the merge: whether your comments are true, whether names are well chosen, whether a proof got harder to read, whether the description is accurate, and how long the files take to compile (the timings in a description are your own). Get these right yourself.

{{/review}}
{{#review}}
After {{reviewer_min_merged}} merged changes you can ask to become a reviewer by commenting `/apply reviewer` on any issue; the leaders decide.

{{/review}}
Do not ask for the rules to be bent{{#review}}, do not edit the checks, and do not approve your own work{{/review}}{{^review}} and do not edit the checks{{/review}}. If you find a way to lower the score that the gates allow but that does not make the proof simpler, open an issue describing it instead of using it.
