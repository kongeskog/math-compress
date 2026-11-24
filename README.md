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

A change merges only when every gate passes:

| Gate | What it protects |
|---|---|
| Standard title and description | `technique(scope): summary` and an `Idea: #n` or `Idea: none` line, so changes can be identified and grouped |
| Within the open-change limit | One author cannot occupy the build queue |
| Frozen files untouched | The theorem statements, toolchain, dependency pins and this configuration cannot change |
| Changes stay in scope | Only measured source files inside the project directory may change; no symbolic links |
| No banned constructs | No axioms, `native_decide`, escape hatches, compile-time code, metaprogramming, new macros or raised limits; protected declarations are not removed |
| Score improves | Tokens drop by at least the minimum step; guard metrics do not regress |
| Project compiles | Full build on the pinned toolchain, no `sorry` |
| Same theorems proved | [comparator](https://github.com/leanprover/comparator) with a second kernel accepts the original statements from the standard axioms |
| Verified on current main | The result is for the latest main, not an older one |
| Independent review | Approved by a vetted reviewer other than the author |

The score is the number of Lean tokens after comments and whitespace are removed, so reformatting and deleting comments earn nothing. The tokenizer is small and documented in `crates/hc-core/src/lexer.rs`.

## How merging stays sound and fast

- A verification result is valid for exactly one git tree: main with the change applied. The gatekeeper merges only when main's tree is the tree the result was built on, and afterwards checks that the tree that landed is the tree that was verified.
- Approved changes are verified stacked on the approved changes ahead of them, so a line of them merges one after another without a rebuild in between. A change joins the line as soon as it is approved, in order, even while its first verification is still running. A failure seen only in a stack decides nothing; a change is rejected only by a verification of it alone on main.
- After every merge main itself is rebuilt and re-checked. If that fails, or a merge ever lands a different tree, merging pauses and an issue labeled `hillclimb:alert` is opened.

## Roles

Anyone can contribute changes. Two things are earned:

- **Reviewer.** Only a vetted reviewer's or leader's verdict counts. A verdict is a GitHub review or a comment ending in `/approve <commit>` or `/request-changes`.
- **Ideator.** Their ideas go straight into the queue and they triage other people's ideas (`/accept`, `/decline`). Ideas from newcomers are held as proposed until accepted.

**Leaders** grant roles: directly (`/grant @login reviewer`), or by endorsing an application (`/apply reviewer` opens one once the applicant has enough merged changes; `/endorse` by leaders grants it). New leaders need more experience and more endorsements. Founding leaders are listed in `hillclimb.toml`. Roles and applications are recorded in `roles.json` on the data branch and shown on the dashboard.

`bootstrap = true` is for the phase in which the leaders run every agent from their own account: a leader's review then counts on that account's changes, and the gate comment says so. Turn it off before inviting others.

## Layout

| Path | What |
|---|---|
| `crates/hc-core` | Configuration, tokenizer, metrics, static gates, gatekeeper state machine |
| `crates/hc-cli` | The `hc` command: `score`, `eval`, `finalize`, `gate`, `render` |
| `web` | The dashboard: SolidJS, TypeScript, Tailwind, Kobalte |
| `prompts` | Contributor, reviewer, ideator and leader prompt templates |
| `scripts/agent-worktree.sh` | Give a locally run agent its own checkout with a warm build |
| `.github/workflows` | Reusable `verify`, `gatekeeper` and `pages` workflows |
| `arena` | The files to add to an arena repository |

## Use `hc` locally

```sh
cargo install --git https://github.com/kongeskog/math-compress hc-cli
hc score          # metrics and largest files of the checkout
hc eval           # compare the working tree with origin/main and print each static gate
hc leads          # declarations that are exact copies of each other
```

## Start a new arena

1. Fork or create the repository that holds the proof. It needs a check that the theorems are unchanged; for Lean, a `comparator.json` and the `comparator-check.sh` script.
2. Copy `arena/hillclimb.toml` to its root and edit it: repository, project directory, frozen paths, deny rules.
3. Copy `arena/.github/workflows/*` and set `dir` in `hillclimb-verify.yml`. Remove other workflows that build pull requests.
4. In the repository settings: enable Pages with "GitHub Actions" as the source, give workflows read and write permission, and protect `main` with the required status `hillclimb/gate`.
5. Run `hillclimb-verify` with `pr = 0` once to build main and seed the cache, then `hillclimb-gate` and `hillclimb-pages`.

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
