# What we learned from the first arena

The first arena shortened a Lean 4 proof of VP ≠ VNP (321 files, 928,161 tokens) to 833,768 tokens,
a reduction of 10.17%, in 82 merged changes over two days. Every number below comes from that
arena's `attempts.jsonl` unless it says otherwise. This page records what produced the reduction,
what cost time for nothing, and what to do differently when starting the next arena.

## The result in numbers

| | Start | End |
|---|--:|--:|
| Tokens | 928,161 | 833,768 |
| Lines of code | 58,797 | 53,372 |
| Declarations | 4,610 | 4,353 |
| Files | 321 | 314 |

- 82 changes merged, 2 rejected for a real reason (both the same change, for one line over the
  length limit), 2 rejected on purpose to test the pipeline.
- The median merged change saved 591 tokens. The ten largest changes account for 53% of the
  total; the largest single change for 16%.
- From opening a pull request to its merge: 25 minutes at the median, 49 minutes at the 90th
  percentile. A verification build took about 9 minutes.
- Of 55 ideas filed, 51 were closed as done or dead ends.

### Where the tokens came from

| Technique | Changes | Tokens | Per change |
|---|--:|--:|--:|
| `restructure` | 24 | 33,681 | 1,403 |
| `dead-code` | 6 | 27,469 | 4,578 |
| `shorten` | 32 | 22,180 | 693 |
| `merge` | 13 | 5,244 | 403 |
| `library` | 4 | 4,364 | 1,091 |
| `generalize` | 3 | 1,455 | 485 |

Deleting and restating beat local polishing by a wide margin per change. Each change costs the
same review and the same build whatever its size, so the per-change column is the one that matters.

## What worked, in order of yield

1. **Delete what the final theorems do not reach.** A reachability pass from the protected
   theorems through the constants each proof term uses found whole files that nothing needed.
   One change of this kind saved 14,853 tokens. Lemmas that are used only through a `simp` set
   look unreached to such a pass and must be kept; list them once and exclude them.
2. **Delete what is reached but unused inside a statement.** Fields of a structure nobody
   projects, conjuncts of a conclusion nobody destructures, hypotheses and arguments nobody
   supplies anything interesting for. Two changes of this kind saved 6,019 and 4,712 tokens,
   because each unused conjunct takes its whole proof with it.
3. **Prove one thing through another instead of twice.** The proof had an exact and an
   approximate version of one construction; routing the exact result through the approximate
   one deleted a file and 7,227 tokens. Look for pairs of developments with a bridge between them.
4. **Remove a layer of machinery by moving information to where it is used.** The largest
   late change (5,561 tokens, 34 files) removed an interpreter that existed only to account for
   something the programs could not see; once the programs returned that information as part of
   their value, the interpreter and its theorems went.
5. **Restate a bound so that conversions disappear.** Stating one theorem with products
   instead of quotients of natural numbers removed the lemmas that only got in and out of floors.
6. **Inline wrappers and single-use lemmas** whose statement is as long as their proof.
7. **Replace the tail of a tactic block by one closing tactic.** A tool that tries a closing
   tactic at every step of every tactic block found these mechanically; four changes saved
   6,017 tokens. This needs rules (see below) and it is the least satisfying kind of saving.
8. **Derive instead of hand-writing instances**, and use the library lemma where one exists.

## What did not work

- **Looking for an architectural shortcut.** Three long studies of the proof's structure each
  concluded that no different route was shorter, and estimated a floor about 10% below the start.
  The arena ended inside that estimate. A study is worth one run to set expectations and to
  produce a ranked list of structural changes; repeating it did not find more.
- **A shorter-looking route that needs new groundwork.** An alternative argument for one
  section would have cost about 2,500 tokens of preparation to save 1,600. Estimate the cost of
  what the new route needs before starting.
- **Narrowing a hypothesis to what its consumer uses.** Estimated at 200 to 400 tokens,
  measured at 91: the narrower statement moved the work to two consumers that then had to prove
  they were inside the narrower case.
- **Replacing explicit arithmetic by `nlinarith`.** It shortened the text and multiplied the
  elaboration time of the file (in one case from 7 to 42 seconds). Each time the explicit steps
  had to be put back, at a cost of 150 to 400 tokens of the saving.
- **Inlining a lemma into a long proof without clearing its context.** One inlined proof made
  its file 52% slower because the extra hypotheses slowed every later `omega`. One `clear` line
  fixed it.
- **Trusting a tool's estimate.** The closing-tactic tool's hits were upper bounds: a bit under
  half of the listed tokens were realized, about a fifth of the small hits fell under the
  per-site minimum once measured, and some failed when applied.
- **Small sweeps.** Unused names, option lines that no longer change anything, unneeded
  `open` lines: each sweep touched dozens of files for 200 to 300 tokens.
- **Many small pull requests early on.** The first 56 changes saved 31,211 tokens; the next 26
  saved 63,182. The difference was larger, structural changes and the decision described next.

## The decision that mattered most

The arena began by protecting a few hundred intermediate statements that mirror the paper, so
that the formal text stayed readable side by side with it. The owner then decided that the goal
was the shortest proof of the final theorems and removed that protection. Everything listed
under 1 to 4 above became possible only then. Decide at the start which of the two goals an
arena has, and say so in `hillclimb.toml` and the prompts: a protected skeleton and a shortest
proof are different projects.

## Rules that had to be added along the way

- **Elaboration time.** A token score rewards heavy automation. The arena settled on: a changed
  file may be at most a tenth slower than on main, measured by interleaved runs and medians.
- **Closing tactics.** Search tactics may replace a tail only with: a limit per file, a minimum
  saving per site, a looser time limit for those files, a specific tactic preferred where one
  works, and a one-line comment at the site saying what the removed lines used. Where a chain of
  inequalities is replaced, the comment carries the argument.
- **No new warnings, and no option deleted to hide one.**
- **A minimum size per change** well above the gate's 25 tokens, because review and build cost
  the same for every change. 300 tokens worked; it was lowered for finished, measured work.
- **At most about 20 files per change**, with a second reviewer above that.

## What reviewers asked for

Nearly every request for changes was about comments, names or layout, not about proofs, and
cost no tokens. The same few items came back again and again; an author who checks them first
saves a review round:

- A deleted declaration is still named in a comment, a doc comment or a module doc.
- A comment at a changed site names a hypothesis that is not in scope there, or says something
  the new proof no longer does.
- A hypothesis keeps a name that described the old statement.
- A doc comment describes the statement as it was before the change.
- Counts in the description do not match the diff.
- Timings are missing for some changed files.
- A line is longer than the limit, or a line was broken in the middle of a factor.

Reviews of this kind took one to two minutes each and a second look at the fixed commit took
under a minute. Asking the same reviewer to look again was much cheaper than a fresh review.

## Running the work

- **Test a large change in scratch first.** For each structural idea, one case or one file was
  ported before committing to the whole change, with a stated threshold for going on. This
  stopped two ideas early and confirmed two others.
- **Reserve files for a large change, and stack on it.** While a 34-file change was in
  progress its files were off limits to everyone else. Smaller changes that needed those files
  were prepared on top of its branch and opened after it merged. Nothing conflicted.
- **Slice mechanical work by folder**, so that several people can work on one idea at once.
- **Hold back the part that overlaps an open change** instead of forcing a conflict, and write
  down exactly what was held back.
- **Few, larger changes.** With a fixed number of people or agents working at once, the limit
  on throughput was review and attention, not the build.
- **Keep working notes and tool output somewhere durable.** A reboot erased a temporary
  directory that held the plan, the analysis tools and a long tool run. Commit work in progress
  and push branches as you go.

## The pipeline

- Static gates before the build paid off: format, frozen files, scope and the deny list reject
  in seconds.
- Verifying approved changes stacked on each other let a line of them merge without a rebuild
  between each. This removed the build as the bottleneck.
- A check that could not be set up is not a failed gate. Two false alarms came from downloads
  failing during setup; the fix was three outcomes for a check (pass, fail, no result) with a
  retry on the third, and retries around downloads.
- Runs cancelled because a newer one replaced them showed as failed checks on merged changes.
  They are harmless; letting the pull-request event only request a gate run removed them.
- The title format stopped one change for hours because of a space inside the scope. The line
  length gate rejected one change after it had been approved. Both are static: run `hc eval`
  again after the last edit, and check the added lines before opening.

## For the next arena

- Decide first whether intermediate statements are protected.
- Run the reachability pass and the unused-parts pass before anything else; they are the
  cheapest large savings and they change what everything after them looks like.
- Run one structural study early, for a floor estimate and a ranked list.
- Set the minimum size per change and the time rule on day one.
- Give the contributor prompt the list under "What reviewers asked for".
- Expect the rate to fall as the estimated floor comes near; stop when changes cost more
  attention than they return.

## Not yet in this library

The analysis tools that found most of the large savings were written as scratch Lean programs
for the first arena and are not part of `hc`: reachability from the protected theorems, unused
conjuncts and fields, declarations with identical statements, unused arguments, and the tool
that tries a closing tactic at each step of a tactic block. `hc leads` covers only exact copies.
