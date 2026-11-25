/-
Twins: reached declarations of a Lake project whose statements are the same or nearly so.

Usage, from inside the Lake project directory (the one with `lakefile.toml`):

    lake env lean --run Twins.lean [--max-diff k] [--min-size n] [--everywhere] \
      <module>… -- <root>…

Options come first; then, as for `Reach.lean`, the modules to import before `--` and the root
constants after it. Only the project's own user-written theorems and definitions that the roots
reach are compared (instances, structures and generated declarations are skipped). A theorem is
compared by its type; a definition by its type together with its value.

Comparison is on elaborated terms: annotations are dropped and universe parameters are renamed in
order of first occurrence, so names of bound variables and the order of universe parameters do
not matter (binder annotations are ignored too, as by `Expr.eqv`).

* Exact twins: equal terms. A hash, then an exact check.
* Near twins: terms that become equal when every constant and literal is replaced by a hole,
  and that differ in at most k distinct pairs of constants or literals (default 2). For example
  the same lemma about `Nat.succ` and `Nat.pred`, or about the two fields of a structure: a
  candidate for one statement with a parameter. Only pairs in the same module or the same
  namespace are compared unless `--everywhere` is given.
* Terms smaller than `--min-size` expression nodes (default 15) are skipped as trivia.

Output, one record per line, fields separated by tabs:

    twin <group> <how> <module> <name> <kind> <first line> <last line> <difference>

`how` is `exact` or `near`; all members of a group share its number. A near group is a pair; its
second member's `difference` lists the pairs `a ↔ b` in which the two differ (`-` otherwise).
Lines starting with `# ` are a summary.

Caveats. Equal statements do not mean that one proof can serve both places: implicit arguments
may have to be stated, and a definition's twin may be used under its own name by `simp` or
`unfold`. Near twins are candidates for a parametrized statement only; the parameter may not be
expressible (for example two constants of different types). The test is: change, build, compare.
-/
import Lean

open Lean Meta

namespace Twins

def modulePath (mod : Name) : System.FilePath :=
  let parts := mod.components.map fun
    | .str _ s => s
    | n => n.toString
  (System.mkFilePath parts).addExtension "lean"

def projectModules (env : Environment) : IO (Array Bool) :=
  env.header.moduleNames.mapM fun mod => (modulePath mod).pathExists

def inProject (env : Environment) (isProj : Array Bool) (n : Name) : Bool :=
  match env.getModuleIdxFor? n with
  | some i => isProj.getD i.toNat false
  | none => false

def moduleOf (env : Environment) (n : Name) : Name :=
  (env.getModuleIdxFor? n >>= (env.header.moduleNames[·.toNat]?)).getD .anonymous

def directDeps (ci : ConstantInfo) : Array Name := Id.run do
  let mut out := ci.type.getUsedConstants
  if let some v := ci.value? (allowOpaque := true) then
    out := out ++ v.getUsedConstants
  match ci with
  | .inductInfo i => out := out ++ i.ctors.toArray ++ i.all.toArray
  | .ctorInfo c => out := out.push c.induct
  | .recInfo r =>
    out := out ++ r.all.toArray
    for rule in r.rules do
      out := out.push rule.ctor ++ rule.rhs.getUsedConstants
  | _ => pure ()
  return out

def reachable (env : Environment) (isProj : Array Bool) (roots : Array Name) : NameSet := Id.run do
  let mut seen : NameSet := {}
  let mut stack := roots
  while !stack.isEmpty do
    let n := stack.back!
    stack := stack.pop
    if seen.contains n then continue
    seen := seen.insert n
    let some ci := env.find? n | continue
    for d in directDeps ci do
      if !seen.contains d && inProject env isProj d then
        stack := stack.push d
  return seen

def generatedSuffixes : List String :=
  ["rec", "recOn", "casesOn", "noConfusion", "noConfusionType", "below", "ibelow", "brecOn",
   "binductionOn", "sizeOf_spec", "injEq", "inj", "eq_def", "induct", "mutual_induct",
   "fun_cases", "ctorIdx", "toCtorIdx", "splitter", "ext_iff", "ctorElim", "ctorElimType"]

def numbered (s pre : String) : Bool :=
  s.startsWith pre && (s.drop pre.length).all fun c => c.isDigit || c == '_'

def internalComponent (s : String) : Bool :=
  s.startsWith "_" || numbered s "eq_" || numbered s "match_" || numbered s "proof_"
    || numbered s "omega_"

def userWritten (env : Environment) (n : Name) (ci : ConstantInfo) : Bool := Id.run do
  let u := (privateToUserName? n).getD n
  for c in u.components do
    match c with
    | .str _ s => if internalComponent s then return false
    | _ => return false
  if let .str _ last := u then
    if generatedSuffixes.contains last then return false
  if ci matches .ctorInfo _ || ci matches .recInfo _ || ci matches .quotInfo _ then return false
  if isAuxRecursor env n || isNoConfusion env n || isMatcherCore env n then return false
  if env.isProjectionFn n then return false
  if isInstanceCore env n.getPrefix then return false
  return true

def userName (n : Name) : Name := (privateToUserName? n).getD n

/-! ### Normal forms -/

/-- Drop annotations, rename universe parameters in order of first occurrence. -/
def normalize (e : Expr) : CoreM Expr := do
  let e ← Core.transform e (post := fun e => match e with
    | .mdata _ b => return .done b
    | _ => return .continue)
  let ps := (collectLevelParams {} e).params.toList
  let us := (List.range ps.length).map fun i => mkLevelParam (Name.mkSimple s!"u{i}")
  return e.instantiateLevelParams ps us

def hole : Name := `_twins_hole

/-- Every constant and literal replaced by one hole. -/
def skeleton (e : Expr) : CoreM Expr :=
  Core.transform e (post := fun e => match e with
    | .const .. | .lit .. => return .done (.const hole [])
    | _ => return .continue)

/-- Number of expression nodes, counted as a tree, up to `cap`. -/
partial def size (e : Expr) (cap : Nat := 100000) : Nat :=
  go e 0
where
  go (e : Expr) (acc : Nat) : Nat :=
    if acc ≥ cap then acc else
    match e with
    | .app f a => go a (go f (acc + 1))
    | .lam _ t b _ | .forallE _ t b _ => go b (go t (acc + 1))
    | .letE _ t v b _ => go b (go v (go t (acc + 1)))
    | .mdata _ b | .proj _ _ b => go b (acc + 1)
    | _ => acc + 1

def leafStr : Expr → String
  | .const c _ => toString (userName c)
  | .lit (.natVal n) => toString n
  | .lit (.strVal s) => s.quote
  | e => toString e

/-- The distinct pairs of leaves in which two terms of the same skeleton differ; `none` when
there are more than `k` or the walk is too long. -/
partial def diffs (a b : Expr) (k : Nat) : Option (Array (Expr × Expr)) :=
  match go a b (#[], 0) with
  | some (out, _) => some out
  | none => none
where
  go (a b : Expr) (st : Array (Expr × Expr) × Nat) : Option (Array (Expr × Expr) × Nat) := do
    let (out, steps) := st
    if steps > 200000 then none
    let st := (out, steps + 1)
    match a, b with
    | .app f x, .app g y => go x y (← go f g st)
    | .lam _ t c _, .lam _ u d _ | .forallE _ t c _, .forallE _ u d _ => go c d (← go t u st)
    | .letE _ t v c _, .letE _ u w d _ => go c d (← go v w (← go t u st))
    | .proj _ i x, .proj _ j y => if i == j then go x y st else none
    | .const .., .const .. | .lit .., .lit .. | .const .., .lit .. | .lit .., .const .. =>
      let same := match a, b with
        | .const c _, .const d _ => c == d
        | _, _ => a == b
      if same then return st
      if out.any (fun (p, q) => leafStr p == leafStr a && leafStr q == leafStr b) then return st
      if out.size ≥ k then none
      return (out.push (a, b), steps + 1)
    | _, _ => if a == b then return st else none

/-! ### Main -/

structure Cand where
  name : Name
  mod : Name
  kind : String
  key : Expr
  skel : Expr
  first : Nat
  last : Nat
  deriving Inhabited

structure Opts where
  maxDiff : Nat := 2
  minSize : Nat := 15
  everywhere : Bool := false

def run (o : Opts) (imports roots : Array Name) : IO Unit := do
  let env ← importModules (imports.map ({ module := · })) {} (loadExts := true)
  let isProj ← projectModules env
  for r in roots do
    unless env.contains r do throw <| IO.userError s!"unknown root constant {r}"
  let reached := reachable env isProj roots
  let marker := mkConst `_twins_def
  let act : CoreM (Array Cand) := do
    let mut cands := #[]
    for (n, ci) in env.constants.map₁.toList do
      unless inProject env isProj n && reached.contains n && userWritten env n ci do continue
      if isInstanceCore env n then continue
      let (kind, raw) ← match ci with
        | .thmInfo t => pure ("theorem", t.type)
        | .defnInfo d =>
          if isStructure env n || isClass env n then pure ("", d.type)
          else pure ("def", mkApp2 marker d.type d.value)
        | _ => pure ("", ci.type)
      if kind.isEmpty then continue
      let key ← normalize raw
      if size key < o.minSize then continue
      let some r ← findDeclarationRanges? n | continue
      let skel ← skeleton key
      let c : Cand := { name := n, mod := moduleOf env n, kind := kind, key := key, skel := skel,
                        first := r.range.pos.line, last := r.range.endPos.line }
      cands := cands.push c
    return cands
  let (cands, _) ← act.toIO { fileName := "<twins>", fileMap := default, maxHeartbeats := 0 }
    { env }
  let cands := cands.qsort fun a b =>
    a.mod.toString < b.mod.toString || (a.mod == b.mod && a.first < b.first)
  let out ← IO.getStdout
  let line (g : Nat) (how : String) (c : Cand) (note : String) : IO Unit :=
    out.putStrLn s!"twin\t{g}\t{how}\t{c.mod}\t{userName c.name}\t{c.kind}\t{c.first}\t\
      {c.last}\t{note}"
  -- Exact twins.
  let mut exact : Std.HashMap Expr (Array Nat) := {}
  for h : i in [0:cands.size] do
    let k := cands[i].key
    exact := exact.insert k ((exact.getD k #[]).push i)
  let mut group := 0
  let mut nExact := 0
  let mut exactOf : Std.HashMap Nat Nat := {}
  let groups := (exact.toArray.filter (·.2.size ≥ 2)).qsort fun a b => a.2[0]! < b.2[0]!
  for (_, members) in groups do
    group := group + 1
    nExact := nExact + 1
    for i in members do
      exactOf := exactOf.insert i group
      line group "exact" cands[i]! "-"
  -- Near twins: same skeleton, few differences.
  let mut bySkel : Std.HashMap Expr (Array Nat) := {}
  for h : i in [0:cands.size] do
    let k := cands[i].skel
    bySkel := bySkel.insert k ((bySkel.getD k #[]).push i)
  let mut nNear := 0
  let mut tooBig := 0
  for (_, members) in bySkel.toArray.qsort (fun a b => a.2[0]! < b.2[0]!) do
    if members.size < 2 then continue
    if members.size > 400 then tooBig := tooBig + 1; continue
    for a in [0:members.size] do
      for b in [a+1:members.size] do
        let x := cands[members[a]!]!
        let y := cands[members[b]!]!
        if x.kind != y.kind then continue
        if !o.everywhere && x.mod != y.mod && x.name.getPrefix != y.name.getPrefix then continue
        if (exactOf[members[a]!]?).isSome && exactOf[members[a]!]? == exactOf[members[b]!]? then
          continue
        match diffs x.key y.key o.maxDiff with
        | some ds =>
          if ds.isEmpty then continue
          group := group + 1
          nNear := nNear + 1
          let note := ", ".intercalate (ds.toList.map fun (p, q) => s!"{leafStr p} ↔ {leafStr q}")
          line group "near" x "-"
          line group "near" y note
        | none => pure ()
  out.putStrLn s!"# {cands.size} reached theorems and definitions of at least {o.minSize} nodes"
  out.putStrLn s!"# exact groups: {nExact}"
  let scope := if o.everywhere then "anywhere" else "same module or namespace"
  out.putStrLn s!"# near pairs (at most {o.maxDiff} differences, {scope}): {nNear}"
  if tooBig > 0 then
    out.putStrLn s!"# skeletons shared by more than 400 declarations, not compared: {tooBig}"

end Twins

/-- Options, then `<module>… -- <root>…`. -/
partial def parseArgs (o : Twins.Opts) : List String → Option (Twins.Opts × Array Name × Array Name)
  | "--max-diff" :: k :: rest => k.toNat?.bind fun k => parseArgs { o with maxDiff := k } rest
  | "--min-size" :: k :: rest => k.toNat?.bind fun k => parseArgs { o with minSize := k } rest
  | "--everywhere" :: rest => parseArgs { o with everywhere := true } rest
  | args =>
    match args.span (· != "--") with
    | (mods, "--" :: roots) =>
      if mods.isEmpty || roots.isEmpty then none
      else some (o, mods.toArray.map String.toName, roots.toArray.map String.toName)
    | _ => none

unsafe def main (args : List String) : IO UInt32 := do
  let some (o, imports, roots) := parseArgs {} args
    | IO.eprintln "usage: lake env lean --run Twins.lean [--max-diff k] [--min-size n] \
        [--everywhere] <module>... -- <root constant>..."
      return 2
  initSearchPath (← findSysroot)
  enableInitializersExecution
  try
    Twins.run o imports roots
    return 0
  catch e =>
    IO.eprintln s!"twins: {e}"
    return 1
