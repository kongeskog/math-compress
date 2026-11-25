# math-compress

A harness for running an open, distributed hillclimb on a machine-checked proof: many independent agents propose changes that make the proof smaller, automated gates decide what merges, and a public dashboard shows the score going down.

The first arena is the Lean 4 formalization of VP ≠ VNP from [anthropics/formal-math](https://github.com/anthropics/formal-math/tree/main/vp-vnp). Baseline: 928,161 tokens in 321 files.

Everything runs on GitHub. There is no server and no database.

## How it works

```
contributor's agent ──pull request──▶ arena repository (a fork of the proof)
                                         │
                 hillclimb-verify        │  static gates (seconds), then a full build in a
                 (pull_request_target)   │  network-less container and an independent proof check
                                         ▼
                 hillclimb-gate          │  reads results and reviews, merges what passed every
                 (events + schedule)     │  gate, keeps the ideas queue, writes state to the
                                         ▼  `hillclimb-data` branch
                 GitHub Pages dashboard  ◀─ polls the data branch and the public GitHub API
```

A change merges only when every gate passes. What a failed gate means is a property of the gate, set in one table (`GATES` in `crates/hc-core/src/model.rs`):

| Gate | What it protects | If it fails |
|---|---|---|
| Standard title and description | `technique(scope): summary`, an `Idea: #n` or `Idea: none` line, and a valid `Depends: #n` line if there is one, so changes can be identified and grouped | needs a fix |
| Within the open-change limit | One author cannot occupy the build queue | needs a fix |
| Frozen files untouched | The theorem statements, toolchain, dependency pins and this configuration cannot change | rejected |
| Changes stay in scope | Only measured source files inside the project directory may change; no symbolic links | rejected |
| No banned constructs | No axioms, `native_decide`, escape hatches, compile-time code or metaprogramming (the `forbid` rules); protected declarations are not removed | rejected |
| No more long lines or exceptions | The number of lines over the length limit, and of constructs counted by `no_increase` rules (macros, raised limits) or above their caps, does not grow | needs a fix |
| Score improves | Tokens drop by at least the minimum step; guard metrics do not regress | needs a fix |
| Project compiles | Full build on the pinned toolchain, no `sorry` | rejected |
| Same theorems proved | [comparator](https://github.com/leanprover/comparator) with a second kernel accepts the original statements from the standard axioms | rejected |
| Verified on current main | The result is for the latest main, not an older one | waits, is verified again |
| Independent review | Approved by a vetted reviewer other than the author | waits |

**Rejected** is a verdict on a commit: the attempt is written to the attempts log with its reason and the pull request is labeled `hillclimb:rejected`; a new push starts a new attempt. **Needs a fix** is not a verdict: the pull request stays open, is labeled `hillclimb:needs-fix`, its `hillclimb/gate` status fails with "Needs a fix: …", the bot's comment lists exactly what to repair, and nothing is written to the attempts log. If a commit fails gates of both kinds it is rejected. A gate the table does not know counts as a verdict.

The score is the number of Lean tokens after comments and whitespace are removed, so reformatting and deleting comments earn nothing. The tokenizer is small and documented in `crates/hc-core/src/lexer.rs`.

### A change built on another change

A pull request whose description has a line `Depends: #n`, where #n is an open pull request against the same branch, is built on #n: its branch contains #n's commits. It is judged as the difference from #n:

- Its static gates (score, scope, the file count behind the large-change rule, banned constructs, line length) are evaluated against main with #n applied, and the bot's comment says "Depends on #n" and what the metrics were measured against. `hc eval --depends-ref <ref of #n>` gives the same numbers locally, and `hc precheck --depends-ref <ref of #n>` runs every check before a pull request against #n.
- It is verified with #n applied first, by the same mechanism that verifies a line of approved changes, and it can be reviewed while #n is open.
- It never merges before #n. When #n is approved and in the line, it joins the line behind it and the two merge one after the other; otherwise it waits outside the line and holds nobody up.
- Its branch must contain #n's head commit, and the bot's comment names that commit ("judged on top of #n at `…`"). Then what it adds to #n is fixed by the commits themselves, and is what a reviewer of the difference read. If #n is pushed to, the format gate fails (needs a fix) until its author merges #n's branch into it and pushes, which makes a new commit that needs a new approval. If the branch already contains #n's new commit, it is only out of date: it is verified again once approved, and approvals given before #n's push have to be given again.
- If #n merges as another commit than the one it was judged on, or as one its branch does not contain, it cannot merge as it is: the format gate says to merge main into the branch and push. The new commit is an ordinary change against main, judged as a whole, and needs its own approval. Without this rule, something #n dropped in a revision would come back with the dependent: its side of the merge has it, main's does not, and no reviewer of the difference ever read it.
- If #n is rejected, needs a fix or conflicts, it waits and says why. A failure of its own run decides nothing until everything it is built on has passed on its own at the commit that was applied: before that the failure may be theirs. If #n is closed without merging, its format gate fails until its author rebases it onto main, removes the line and pushes.
- Chains work the same way (#3 on #2 on #1), up to ten deep. A line that names an issue, a draft, a closed or unknown number, the pull request itself or a circle fails the format gate with the reason. A line inside an HTML comment is not part of the description and is ignored.
- The line belongs to the commit. The gatekeeper records, for every commit of every pull request, when it first judged it and on top of which commits of which changes (`judged` in the ledger). Removing or changing the line without a new push then fails the format gate, so an approval given for the difference from #n cannot be turned into an approval of both. The record is kept while the pull request has not merged, also while it is a draft, closed or retargeted, so it cannot be shed by taking the pull request out of sight for a pass. The ledger keeps at most 2,000 such records beyond those of open pull requests, oldest dropped first; a commit whose record is gone is simply judged for the first time again.
- An approval counts only if it was given after the gatekeeper first judged the commit (after a push to what it is built on: after it judged it on top of the new commit). Every counted approval was therefore given under a bot comment whose statement of what the commit is built on cannot change. A reviewer who approved earlier, for example while the pull request was a draft the gatekeeper does not look at, is told on the pull request to approve again.

### Files reserved by an idea

An idea may have a **Reserves** section listing the paths or globs it will rewrite, in backticks. While the idea is claimed, the bot's comment on every other open pull request that touches one of them carries a warning: "touches files reserved by idea #n (claimed by @x): …; coordinate there or expect to merge main after it lands". It is a warning only: it fails no gate and blocks no merge. The claimant's pull request for that idea (`Idea: #n`) is exempt, and so are the claimant's other changes, except in the bootstrap phase, where all agents share one account and only the pull request for the idea itself is exempt. A change built on another one (`Depends`) is warned only for the files it changes itself, not for those of what it is built on; if that difference cannot be read, the warning says that the list may include them. `hc eval --reservations` prints the reservations and warns locally.

A pattern matches a path from the repository root or from the project directory; one without `*` also covers the folder of that name and the file of that name with any extension (`Paper/Tree` covers `Paper/Tree.lean` and `Paper/Tree/…`). If nothing in the section is in backticks, every word containing `/`, `*` or `.` is taken as a pattern.

### Ideas

An idea is an issue titled `Idea: <technique>(<scope>): <summary>` with the sections **Targets**, **Technique**, **How to check** and **Estimated saving**; a section starts with its name in bold or as a heading. If the estimate is above `ideas.trial_above` tokens (default 1000; 0 turns the rule off), a **Trial** section is required as well: what was ported or measured on one file or case, and the saving measured there. The estimate is the largest number in the Estimated saving section: digits, optionally grouped with commas (`1,200`), with an optional `k` for thousands (`1.5k`); a number right after `#` is an issue reference and is skipped; if the section has no heading, the paragraph that mentions the estimate is read. An idea that is missing something is told once what, and is picked up when edited. Ideas already in the queue are not judged again.

## How merging stays sound and fast

- A verification result is valid for exactly one git tree: main with the change applied. The gatekeeper merges only when main's tree is the tree the result was built on, and afterwards checks that the tree that landed is the tree that was verified.
- Approved changes are verified stacked on the approved changes ahead of them, so a line of them merges one after another without a rebuild in between. A change joins the line as soon as it is approved, in order, even while its first verification is still running, and a change never merges ahead of a line that is being verified, since that would move main under all of it. A failure seen only in a stack decides nothing; a change is decided only by a verification of it alone on main (for a change built on others: on main with exactly those applied).
- A stacked result is merged from only if it lands as the tree it verified. Verification applies the changes ahead as real merges; they land as squashed commits, which do not have their own commits as ancestors. For independent changes both give the same tree. For a change whose branch contains commits of a change ahead of it (`Depends: #n`, or any branch made that way), merging it after the squash is another three-way merge: it conflicts if both edit the same lines, and it can succeed with another tree if the change undoes part of what it is built on. So every run also computes the tree as it will land (`hc landing`: the stack as squashed commits, which must give the very tree the build started from, then the merge), the result records both, and the gatekeeper merges from it only when they are the same tree. Otherwise the change is verified again on main alone once what it was built on has merged, which is the very merge that lands; if that conflicts, its author merges main into the branch and pushes.
- A change with a `Depends` line on an open pull request has no result it can merge from, whatever the trees say: it merges only after everything it is built on has merged, and then by the rule above.
- A pass of the score gate counts only for the minimum it was judged with. Every result records that minimum; one judged with a lower minimum than the one in force for the commit (the configured one, or 1 while a leader's waiver for that commit stands) fails the gate again in the gatekeeper.
- An approval is for one commit, named in full: `/approve` and `/waive-min` take the whole hash, since a prefix could also be the prefix of a commit pushed later. It counts only if given after the gatekeeper first judged that commit (see above).
- After every merge main itself is rebuilt and re-checked. If that fails, or a merge ever lands a different tree, merging pauses and an issue labeled `hillclimb:alert` is opened.

What these rules do not cover:

- **The merge itself.** That a merge lands the verified tree rests on GitHub's squash merge giving the same tree as `git merge` in the verify workflow for the same three inputs. The gatekeeper cannot compute GitHub's result beforehand; it compares the trees after the merge, and a difference pauses merging and raises an alert, by which time the commit is on main.
- **What main gets by other means.** A push to main by someone with the right to push, or a revert made by hand, is not judged by any gate. If content of a merged change is reverted on main by hand, a later change whose branch still has that content can bring it back; this shows in that change's diff against main and in its gates, but nothing flags it as a return.
- **What a reviewer read.** The gatekeeper fixes what an approval is an approval of (one commit, and for a change built on another, the difference from one named commit). It cannot know that the reviewer read that difference rather than something else.
- **Clocks.** "Given after the gatekeeper first judged the commit" compares GitHub's time of the review or comment (whole seconds) with the gatekeeper's clock. An approval within about a second after the first judgement may be asked for again; one given before it never counts.
- **The forge's answers.** Whether a branch contains a commit, and which files a pull request changes, are asked of GitHub. If the first cannot be answered, nothing merges from that commit until it can; if the second cannot, the reservation warning says the check could not be made.
- **A change first seen after what it was built on has merged** is an ordinary change against main: it is judged and reviewed as a whole, whatever its description says.

## Roles

Anyone can contribute changes. Two things are earned:

- **Reviewer.** Only a vetted reviewer's or leader's verdict counts. A verdict is a GitHub review or a comment ending in `/approve <commit>` (the full hash) or `/request-changes`.
- **Ideator.** Their ideas go straight into the queue and they triage other people's ideas (`/accept`, `/decline`). Ideas from newcomers are held as proposed until accepted.

**Leaders** grant roles: directly (`/grant @login reviewer`), or by endorsing an application (`/apply reviewer` opens one once the applicant has enough merged changes; `/endorse` by leaders grants it). New leaders need more experience and more endorsements. Founding leaders are listed in `hillclimb.toml`. Roles and applications are recorded in `roles.json` on the data branch and shown on the dashboard.

**Coordinators** hold no special rights. Whoever runs several workers at once (contributors, reviewers, scouts) can follow `hc prompt coordinator`: what to run before the first change, how to order the queue by what each change returns, how to keep workers out of each other's files, and how to move a change through review.

`bootstrap = true` is for the phase in which the leaders run every agent from their own account: a leader's review then counts on that account's changes, and the gate comment says so. Turn it off before inviting others.

## Commands

Every command is a comment whose line starts with it.

| Command | Who | Where | What it does |
|---|---|---|---|
| `/claim`, `/unclaim` | anyone; the claimant | a queued idea | Take the idea for `ideas.lease_hours`, or release it. While claimed, its **Reserves** are in force |
| `/retry` | the author or a leader | a pull request | Run verification again (with what the change is built on applied, and with a waived minimum if one stands) |
| `/approve <commit>` | reviewers and leaders | a pull request | Approve exactly that commit. The hash in full (the bot's comment prints the line); a short hash does not count and is answered with the line to use |
| `/request-changes <reason>` | reviewers and leaders | a pull request | Block the change until the same reviewer approves |
| `/waive-min <commit> <reason>` | leaders | a pull request | Accept a saving below `score.min_delta` for exactly that commit (the hash in full) |
| `/accept`, `/decline` | ideators and leaders | a proposed idea | Put it into the queue, or close it |
| `/apply <role>`, `/endorse`, `/decline` | anyone with enough merged changes; leaders | any issue; an application | Ask for a role; support or close the application |
| `/grant @login <role>`, `/revoke @login <role>` | leaders | any issue | Give or take a role directly |

`/waive-min` exists because `score.min_delta` is set high: every change costs a review and a build. With a waiver the score gate requires a drop of at least 1 instead; guard metrics and every other gate are unchanged. The waiver names the head commit, as `/approve` does, and for the same reason: a comment is read some time after it is written, and it must not apply to a commit its author had not seen. A later push therefore needs a new waiver. The reason is mandatory; it is shown in the bot's comment and recorded in the attempt. A leader cannot waive for a change from the leader's own account, except in the bootstrap phase, as with reviews. A waiver starts one verification that judges the score with the waived minimum; the result records that, and counts only while the waiver stands.

## Layout

| Path | What |
|---|---|
| `crates/hc-core` | Configuration, tokenizer, metrics, static gates, gatekeeper state machine |
| `crates/hc-cli` | The `hc` command: `score`, `eval`, `finalize`, `gate`, `ops`, `render` |
| `web` | The dashboard: SolidJS, TypeScript, Tailwind, Kobalte. Its Operations page shows where time goes between an idea and a merge, from `ops.json` on the data branch |
| `prompts` | Contributor, reviewer, ideator, leader and coordinator prompt templates (`hc prompt <role>` renders one for an arena) |
| `scripts/agent-worktree.sh` | Give a locally run agent its own checkout with a warm build |
| `.github/workflows` | Reusable `verify`, `gatekeeper` and `pages` workflows |
| `arena` | The files to add to an arena repository |

## Use `hc` locally

```sh
cargo install --git https://github.com/kongeskog/math-compress hc-cli
hc score          # metrics and largest files of the checkout
hc eval           # compare the working tree with origin/main and print each static gate
hc eval --depends-ref origin/their-branch   # for a change built on another open one: compare with that
hc eval --reservations                      # also list files reserved by claimed ideas, and warn (uses `gh`)
hc leads          # declarations that are exact copies of each other
hc precheck       # before a pull request: hc eval, deleted names still mentioned, added lines, open changes to the same files
hc time --base ../main-checkout   # compile time of each changed file there and here, alternating runs, as a table for the description
hc analyze reach  # in a built checkout: declarations the final theorems do not reach, with tokens (candidates: delete, build, compare)
hc analyze unused # structure fields, conjuncts of statements and hypotheses, and binders that nothing reads
hc analyze twins  # theorems and definitions with equal or nearly equal elaborated statements, with the tokens a merge could save
hc analyze tails Nc/A.lean   # steps after which one closing tactic proves the rest of the block (--all, --tactic, --out, --resume)
```

## Start a new arena

1. Fork or create the repository that holds the proof. It needs a check that the theorems are unchanged; for Lean, a `comparator.json` and the `comparator-check.sh` script.
2. Copy `arena/hillclimb.toml` to its root and edit it: repository, project directory, frozen paths, deny rules.
   Three choices in that file shape everything after, so make them before the first change:
   - **What is protected.** By default only the frozen files and the final theorems: anything those theorems do not need may be deleted and intermediate statements may change. To keep a proof that mirrors a paper statement by statement, add `[[protect]]` rules. These are different projects; the first arena got two thirds of its reduction after dropping protection.
   - **The smallest change worth accepting**, `score.min_delta`. Each change costs a review and a full build whatever its size.
   - **Conduct**, the `[conduct]` section: how much slower a changed file may compile (`max_slowdown_pct`, default 10), and whether tactics that search for a proof may replace explicit steps to save tokens (`[conduct.search_tactics]`, off unless `enabled = true`, with a limit per file, a minimum saving per place and a required comment).
3. Copy `arena/.github/workflows/*` and set `dir` in `hillclimb-verify.yml`. Remove other workflows that build pull requests.
4. In the repository settings: enable Pages with "GitHub Actions" as the source, give workflows read and write permission, and protect `main` with the required status `hillclimb/gate`.
5. Run `hillclimb-verify` with `pr = 0` once to build main and seed the cache, then `hillclimb-gate` and `hillclimb-pages`.

### Update an existing arena

An arena calls the reusable workflows at `@main`, so it gets a new gatekeeper and a new verification without a change of its own. For the changes described above (changes built on others, "needs a fix", reservations, waivers, trials) it needs two things:

- In its `hillclimb-verify.yml`, the dispatch input `min_delta` and the line `min-delta: ${{ inputs.min_delta || '' }}` passed to the reusable workflow, as in `arena/.github/workflows/hillclimb-verify.yml`. Until then everything else works; a waiver is answered but its verification cannot be started, which the gatekeeper's log says.
- Optionally `trial_above` under `[ideas]` in `hillclimb.toml` (1000 if absent) and the new pull request template.

Two things change for people at the update. Approvals given before the gatekeeper's first pass with this version do not count, because no commit has a record yet: each open pull request's reviewers are told once to approve again. And `/approve` needs the full hash from then on.

No new permission is needed, and the label `hillclimb:needs-fix` is created the first time it is used. Results written before the update keep the rules they were made under: they carry the count checks inside the banned-constructs gate, and a stacked one is merged from as before, except for a commit that was judged on top of another change (`Depends`), which needs a result that records the tree as it will land.

## Develop

```sh
cargo test
cd web && pnpm install && pnpm dev    # serves demo data from web/public/demo
```

## Security notes

- Verification runs on `pull_request_target`, so workflow definitions and scripts come from main. Pull request content is read as data, or compiled inside a container with no network that can write only to the checkout.
- No job that touches pull request content has a write token or a secret.
- The gatekeeper trusts a result only if it was produced by the verify workflow on the main branch of the arena.
- The deny list is defence in depth, not a sandbox. Soundness rests on the frozen statement files and the independent kernel replay.

## License

Apache-2.0.
