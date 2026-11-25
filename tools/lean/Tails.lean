/-
Tails: places in a file's tactic blocks where one closing tactic proves everything that is left.

Usage, from inside the Lake project directory (the one with `lakefile.toml`), one file per run:

    lake env lean --run Tails.lean [--tactic t]… [--heartbeats n] [--min-chars n] <file.lean>

The file's imports must be built. The file is elaborated with info trees (like compiling it),
then for every tactic block (`by …`, `· …`, `case … => …`, `{ … }`: a tactic sequence) and every
step of it, each candidate tactic (default: `grind`, `simp_all`; `--tactic` replaces the list) is
run on the goals that are open before that step, in that step's context. A hit means: the
candidate closes all of them, without errors and without `sorry`, so the step and everything
after it in the block could be replaced by the candidate. A step that runs several times (inside
`<;>`, `all_goals`, or a block elaborated more than once) is a hit only if the candidate closes
the goals of every run. Each try starts from the state before the step and is limited to
`--heartbeats` thousand heartbeats (default 20000). Only the earliest hit in a block for each
candidate is reported (it replaces the most). Steps whose remaining text is shorter than
`--min-chars` characters (default 25) are not tried. Blocks after whose last step goals remain
are skipped.

Output, one record per line, fields separated by tabs:

    hit  <file> <line> <col> <tactic> <start line> <start col> <end line> <end col> <goals> <runs>
    file <file> <blocks> <steps tried> <tries> <milliseconds>

Lines are 1-based, columns 0-based in characters. The range runs from the start of the step to the
end of the block. `goals` is the largest number of goals open at the step; `runs` the number of
times the step was elaborated. Lines starting with `# ` are a summary.

Hits are upper bounds. The tool tests the goals at a step, not the edited text: after the
replacement the file is elaborated again, and a hit can fail then (names introduced by the
replaced steps, a different elaboration order, `<;>`). Nested hits overlap. Elaboration time is not
measured: a search tactic can make a file much slower to compile. In the first arena a bit under
half of the listed tokens were realized, and about a fifth of the small hits fell under the
minimum saving once measured. The test is: replace, build, compare.
-/
import Lean

open Lean Elab Meta

namespace Tails

structure Opts where
  tactics : Array String := #[]
  heartbeats : Nat := 20000
  minChars : Nat := 25
  file : String := ""

/-- Run `tac` on all goals open before `ti`; true when it closes all of them cleanly. -/
def closes (ci : ContextInfo) (ti : TacticInfo) (tac : Syntax) (hb : Nat) : IO Bool := do
  let ci := { ci with mctx := ti.mctxBefore }
  try
    ci.runMetaM {} do
      withTheReader Core.Context (fun c => { c with maxHeartbeats := hb * 1000 }) do
        withCurrHeartbeats do
          let act : Term.TermElabM Bool := do
            let ctx : Tactic.Context := { elaborator := .anonymous, recover := false }
            let (_, s) ← ((Tactic.evalTactic tac).run ctx).run { goals := ti.goalsBefore }
            Term.synthesizeSyntheticMVarsNoPostponing
            for g in s.goals do
              unless ← g.isAssigned do return false
            for g in ti.goalsBefore do
              let v ← instantiateMVars (mkMVar g)
              if v.hasSorry then return false
            return !(← MonadLog.hasErrors)
          act.run'
  catch _ => return false

def seqKinds : List SyntaxNodeKind :=
  [``Lean.Parser.Tactic.tacticSeq1Indented, ``Lean.Parser.Tactic.tacticSeqBracketed]

/-- The steps of every tactic sequence in a piece of syntax. -/
partial def sequences (stx : Syntax) (acc : Array (Array Syntax)) : Array (Array Syntax) :=
  let acc :=
    if stx.getKind == ``Lean.Parser.Tactic.tacticSeq1Indented then
      acc.push stx[0].getSepArgs
    else if stx.getKind == ``Lean.Parser.Tactic.tacticSeqBracketed then
      acc.push stx[1].getSepArgs
    else acc
  stx.getArgs.foldl (fun acc a => sequences a acc) acc

abbrev Key := String.Pos.Raw × String.Pos.Raw × SyntaxNodeKind

def keyOf (stx : Syntax) : Option Key := do
  let a ← stx.getPos? (canonicalOnly := true)
  let b ← stx.getTailPos? (canonicalOnly := true)
  return (a, b, stx.getKind)

def run (o : Opts) : IO Unit := do
  let start ← IO.monoMsNow
  let path : System.FilePath := o.file
  let input ← IO.FS.readFile path
  let inputCtx := Parser.mkInputContext input path.toString
  let (header, parserState, messages) ← Parser.parseHeader inputCtx
  let opts := (({} : Options).setBool `Elab.async false)
  let (env, messages) ← processHeader header opts messages inputCtx
  let modName := (path.withExtension "").components.foldl (fun n c => n.str c) .anonymous
  let env := env.setMainModule modName
  let cmdState := Command.mkState env messages opts
  let cmdState := { cmdState with infoState.enabled := true }
  let s ← IO.processCommands inputCtx parserState cmdState
  let fileMap := inputCtx.fileMap
  -- Tactic nodes by position and kind.
  let mut nodes : Std.HashMap Key (Array (ContextInfo × TacticInfo)) := {}
  for t in s.commandState.infoState.trees do
    let found := t.foldInfo (init := #[]) fun ci info acc =>
      match info with
      | .ofTacticInfo ti => acc.push (ci, ti)
      | _ => acc
    for (ci, ti) in found do
      if let some k := keyOf ti.stx then
        let old := nodes.getD k #[]
        -- One entry per run: runs differ in the goals they start from.
        unless old.any (fun (_, t') => t'.goalsBefore == ti.goalsBefore) do
          nodes := nodes.insert k (old.push (ci, ti))
  let seqs := s.commands.foldl (fun acc c => sequences c acc) #[]
  let out ← IO.getStdout
  let mut tried := 0
  let mut tries := 0
  let mut hits := 0
  let mut blocks := 0
  for steps in seqs do
    if steps.isEmpty then continue
    let some lastKey := keyOf steps.back! | continue
    let lastRuns := nodes.getD lastKey #[]
    if lastRuns.isEmpty || lastRuns.any (!·.2.goalsAfter.isEmpty) then continue
    let endPos := lastKey.2.1
    blocks := blocks + 1
    let mut done : Array String := #[]
    for step in steps do
      if done.size == o.tactics.size then break
      let some k := keyOf step | continue
      let runs := nodes.getD k #[]
      if runs.isEmpty then continue
      let rest := String.Pos.Raw.extract input k.1 endPos
      if rest.utf8ByteSize < o.minChars then break
      tried := tried + 1
      for tacStr in o.tactics do
        if done.contains tacStr then continue
        if rest.trimAscii.toString == tacStr then continue
        let some (ci0, _) := runs[0]? | continue
        let some tac := (Parser.runParserCategory ci0.env `tactic tacStr).toOption | continue
        let mut ok := true
        for (ci, ti) in runs do
          tries := tries + 1
          unless ← closes ci ti tac o.heartbeats do
            ok := false
            break
        if ok then
          done := done.push tacStr
          hits := hits + 1
          let p := fileMap.toPosition k.1
          let q := fileMap.toPosition endPos
          let goals := runs.foldl (fun m (_, ti) => max m ti.goalsBefore.length) 0
          out.putStrLn s!"hit\t{o.file}\t{p.line}\t{p.column}\t{tacStr}\t{p.line}\t{p.column}\t\
            {q.line}\t{q.column}\t{goals}\t{runs.size}"
  let ms := (← IO.monoMsNow) - start
  out.putStrLn s!"file\t{o.file}\t{blocks}\t{tried}\t{tries}\t{ms}"
  if s.commandState.messages.hasErrors then
    out.putStrLn s!"# {o.file}: the file has elaboration errors; results may be incomplete"
  out.putStrLn s!"# {o.file}: {blocks} blocks, {tried} steps tried, {tries} tries, {hits} hits, \
    {ms} ms"

end Tails

partial def parseArgs (o : Tails.Opts) : List String → Option Tails.Opts
  | "--tactic" :: t :: rest => parseArgs { o with tactics := o.tactics.push t } rest
  | "--heartbeats" :: n :: rest => n.toNat?.bind fun n => parseArgs { o with heartbeats := n } rest
  | "--min-chars" :: n :: rest => n.toNat?.bind fun n => parseArgs { o with minChars := n } rest
  | [file] => some { o with file }
  | _ => none

unsafe def main (args : List String) : IO UInt32 := do
  let some o := parseArgs {} args
    | IO.eprintln "usage: lake env lean --run Tails.lean [--tactic t]... [--heartbeats n] \
        [--min-chars n] <file.lean>"
      return 2
  let o := if o.tactics.isEmpty then { o with tactics := #["grind", "simp_all"] } else o
  initSearchPath (← findSysroot)
  enableInitializersExecution
  try
    Tails.run o
    return 0
  catch e =>
    IO.eprintln s!"tails: {e}"
    return 1
