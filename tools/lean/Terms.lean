/-
Terms: the size of the kernel terms of the project's own declarations.

Usage, from inside the Lake project directory (the one with `lakefile.toml`):

    lake env lean --run Terms.lean <module>… -- <root>…

As for `Reach.lean`: the modules to import before `--`, the root constants after it. For every
reached constant of the project, the type and the value (for theorems, the proof term) are
measured in two ways:

* `dag`: the number of distinct subterms, counting structurally equal subterms once (sharing
  counted once, as a hash-consed representation would store it), within one constant;
* `tree`: the number of nodes when every shared subterm is counted at each occurrence (computed
  exactly over the shared structure, so it can be very large).

Generated constants (`foo._proof_1`, `foo.match_1`, `foo.eq_1`, ...) are added to the
user-written declaration they belong to (the nearest user-written prefix of their name); their
sharing with it is not taken into account. Universe levels and the contents of literals count
as one node each.

Output, one record per line, fields separated by tabs:

    term <module> <declaration> <first> <last> <type dag> <value dag> <value tree> <constants>

Lines starting with `# ` are a summary with the project totals.

Purpose: a second measure beside source tokens. Replacing explicit steps by a search tactic can
lower the tokens while the term stays as large or grows. Caveats: the size of a term depends on
how tactics build it (`simp` and `grind` produce terms with their own overheads), and
auxiliary lemmas that a tactic creates under other names are counted with their owner.
-/
import Lean

open Lean Meta

namespace Terms

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

def owner (env : Environment) (n : Name) : Name := Id.run do
  let mut m := n
  repeat
    if m.isAnonymous then return n
    if let some ci := env.find? m then
      if userWritten env m ci then return m
    m := m.getPrefix
  return n

/-- Distinct subterms (structural) and tree size of a term. -/
def measure (e : Expr) : Nat × Nat := Id.run do
  -- Children first, by an explicit stack; `tree` memoizes the tree size of each subterm.
  let mut tree : Std.HashMap Expr Nat := {}
  let mut stack : Array (Expr × Bool) := #[(e, false)]
  while !stack.isEmpty do
    let (x, done) := stack.back!
    stack := stack.pop
    if tree.contains x then continue
    let kids : Array Expr := match x with
      | .app f a => #[f, a]
      | .lam _ t b _ | .forallE _ t b _ => #[t, b]
      | .letE _ t v b _ => #[t, v, b]
      | .mdata _ b | .proj _ _ b => #[b]
      | _ => #[]
    if done || kids.isEmpty then
      let n := kids.foldl (fun acc k => acc + tree.getD k 0) 1
      tree := tree.insert x n
    else
      stack := stack.push (x, true)
      for k in kids do
        unless tree.contains k do stack := stack.push (k, false)
  return (tree.size, tree.getD e 0)

def run (imports roots : Array Name) : IO Unit := do
  let env ← importModules (imports.map ({ module := · })) {} (loadExts := true)
  let isProj ← projectModules env
  for r in roots do
    unless env.contains r do throw <| IO.userError s!"unknown root constant {r}"
  let reached := reachable env isProj roots
  let mut acc : Std.HashMap Name (Nat × Nat × Nat × Nat) := {}
  for (n, ci) in env.constants.map₁.toList do
    unless inProject env isProj n && reached.contains n do continue
    let o := owner env n
    let (td, _) := measure ci.type
    let (vd, vt) := match ci.value? (allowOpaque := true) with
      | some v => measure v
      | none => (0, 0)
    let (a, b, c, k) := acc.getD o (0, 0, 0, 0)
    acc := acc.insert o (a + td, b + vd, c + vt, k + 1)
  let out ← IO.getStdout
  let act : CoreM Unit := do
    let rows := acc.toArray.qsort fun x y => x.1.toString < y.1.toString
    let mut tTy := 0
    let mut tDag := 0
    let mut tTree := 0
    for (o, (td, vd, vt, k)) in rows do
      let (a, b) ← match ← findDeclarationRanges? o with
        | some r => pure (r.range.pos.line, r.range.endPos.line)
        | none => pure (0, 0)
      out.putStrLn s!"term\t{moduleOf env o}\t{userName o}\t{a}\t{b}\t{td}\t{vd}\t{vt}\t{k}"
      tTy := tTy + td
      tDag := tDag + vd
      tTree := tTree + vt
    out.putStrLn s!"# {rows.size} declarations ({reached.size} reached constants)"
    out.putStrLn s!"# type dag {tTy}, value dag {tDag}, value tree {tTree}"
  let _ ← act.toIO { fileName := "<terms>", fileMap := default, maxHeartbeats := 0 } { env }

end Terms

unsafe def main (args : List String) : IO UInt32 := do
  let (mods, rest) := args.span (· != "--")
  let roots := rest.drop 1
  if mods.isEmpty || roots.isEmpty then
    IO.eprintln "usage: lake env lean --run Terms.lean <module>... -- <root constant>..."
    return 2
  initSearchPath (← findSysroot)
  enableInitializersExecution
  try
    Terms.run (mods.toArray.map String.toName) (roots.toArray.map String.toName)
    return 0
  catch e =>
    IO.eprintln s!"terms: {e}"
    return 1
