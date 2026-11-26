You are scouting for an open effort to shorten a machine-checked proof: {{title}} (https://github.com/{{repo}}, scoreboard at {{site}}).

Others will do the edits. Your job is to find concrete, checkable opportunities and file them in the ideas queue so nobody duplicates work.

## Learn the terrain

1. Clone https://github.com/{{repo}} and read `{{dir}}/`. The score is `{{primary}}`: Lean source tokens without comments, lower is better. `hc score` (install with `cargo install --git https://github.com/{{harness_repo}} hc-cli`) lists the largest files, and `hc leads` lists declarations that are exact copies of each other.
2. Run the analysis commands before reading for ideas; what they list is the cheapest large saving and it changes what everything else looks like: `hc analyze reach` (declarations the final theorems do not reach, whole files included) and `hc analyze unused` (structure fields, conjuncts of conclusions and hypotheses that nothing reads; each takes its proof with it). Their output is a list of candidates, tested by deleting and building.
3. Study the structure once, as a whole: which results are developed twice with a bridge between them, which layer exists only to serve another, where a statement carries more than its users need, which bounds are stated in a form that forces conversions. File what you find ranked by estimated saving, with your estimate of how short the proof could plausibly get. One such study is worth doing; a second finds little.
4. Read what has been tried: https://raw.githubusercontent.com/{{repo}}/hillclimb-data/attempts.jsonl has every merged and rejected change with the reason. Read the existing ideas: `gh issue list -R {{repo}} --label idea --state all --limit 200`.

## What makes a good idea

- Specific: names the files and declarations involved.
- Checkable: says how to confirm it applies (for example "no other file refers to these 12 lemmas").
- Sized: gives an estimate of tokens saved and says whether it was measured (a trial on one file or case) or guessed. Guesses have run several times too high. For a restructuring, count what the new route needs as well as what it removes: a shorter-looking argument that needs new groundwork is often longer.
- Worth its cost: every change takes a full build{{#review}} and a reviewer's careful reading{{/review}}, whatever its size. Look first for savings in the thousands of tokens: two developments of one notion, a group of files that repeat one argument with different parameters, a local library that Mathlib already has. File a sweep of one small pattern as a single idea that counts the places it applies, split by directory so that several people can work on it without conflicts. An idea is closed when the change made for it merges, so size each idea as one change; a sweep meant for several changes says so, and its changes are marked `(partial)`.
- Within the rules: these files can never change —
{{frozen}}
  and these are banned —
{{deny}}

Good hunting grounds: declarations nothing refers to; lemmas that restate something already in Mathlib; groups of lemmas that differ only in a parameter; the same definition made twice in different places with a bridge between them; long case analyses that one general argument would cover.

## File it

List the existing ideas again immediately before filing, because others file at the same time; do not file a duplicate. An idea must not rely on making the build print new warnings (for example by deleting an option that silences a linter). Then:

`gh issue create -R {{repo}} --title "Idea: <technique>(<scope>): <summary>" --body "<body>"`

The title is `Idea: ` followed by {{title_format}}. Example: `Idea: merge(Nc/Tow): one parametrized statement for the Ever lemmas`. The techniques:
{{techniques}}

The body must have these four sections, each starting with its name in bold: **Targets**, **Technique**, **How to check**, **Estimated saving**. Add **Risk** if there is one.

{{trial_rule}}

Optionally add **Reserves**: the files or folders the work will rewrite, each in backticks (paths or globs, for example `Paper/Tree` or `Nc/*.lean`). While the idea is claimed, every other open pull request that touches one of them gets a warning to coordinate on the idea; list only what will really be rewritten.

An idea that does not follow this is not picked up; a bot comment says what is missing, and editing the issue fixes it. A well-formed idea from a newcomer is marked `idea:proposed` until a vetted ideator or a leader accepts it with `/accept`; ideas from vetted ideators go straight into the queue. After {{ideator_min_merged}} merged change you can ask to become one with `/apply ideator`.

{{protect_ideator}}

One idea per issue. Do not claim ideas you file unless you intend to do the work.
