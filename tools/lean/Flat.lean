/-
Flat: lemma boundaries that a flatter proof could remove.

Usage, from inside the Lake project directory (the one with `lakefile.toml`):

    lake env lean --run Flat.lean [--min-size n] [--min-calls n] <module>… -- <root>…

Options first; then, as for `Reach.lean`, the modules to import before `--` and the root
constants after it. Only the project's own declarations that the roots reach count. Generated
declarations (`foo._proof_1`, `foo.match_1`, ...) count as part of the declaration they belong
to (the nearest user-written prefix of their name).

Three analyses, one record per line, fields separated by tabs:

    single   <module> <theorem> <first> <last> <caller> <caller module> <sites> <escapes>
    constarg <module> <theorem> <first> <last> <i> <binder> <calls> <argument>
    fact     <group> <nodes> <module> <declaration> <first> <last> <have name> <statement>
    factpair <group> <caller> <callee>

`single`: a user-written theorem referenced, in types or values, by exactly one other reached
user-written declaration. `sites` is the number of distinct applications of it there (several
sites with different arguments mean that inlining would duplicate its proof); `escapes` is 1
when it also occurs other than applied to all of its binders (passed as a function). The source
range is the whole declaration; the caller of each lemma gives the trees of private developments.

`constarg`: an explicit binder of a reached theorem that receives the same closed term at every
application in reached declarations, at least `--min-calls` times (default 2). Theorems that
occur other than fully applied are skipped. Candidates for specialising the lemma.

`fact`: a proposition established inside a proof by a `have` (a `let`, `let_fun` or beta-redex
whose bound type is a proposition) in two or more different declarations. Fingerprints: the
type with each variable bound outside it replaced by a placeholder that keeps which variable it
is and its type, so facts about different variables of the same types can still collide (a
candidate, not a proof). `nodes` is the size of the subproof in expression nodes (as a tree);
facts whose subproof is below `--min-size` nodes (default 12), or whose fingerprint is below
twice that, are skipped. The `have` name is the bound name (`this` when unnamed); `hc` looks it
up in the declaration's lines. Subproofs are not mapped back to
source ranges; the ranking is by nodes. `factpair` marks members of a group where one
declaration refers to the other (caller and callee prove the same fact).

Caveats. References through notation, `simp` lemmas proved by `rfl` and instances can be missing
(see `Reach.lean`). Everything is a candidate: change, build, compare.
-/
import Lean

open Lean Meta

namespace Flat

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

/-- The user-written declaration a constant belongs to. -/
def owner (env : Environment) (n : Name) : Name := Id.run do
  let mut m := n
  repeat
    if m.isAnonymous then return n
    if let some ci := env.find? m then
      if userWritten env m ci then return m
    m := m.getPrefix
  return n

structure Opts where
  minSize : Nat := 12
  minCalls : Nat := 2

def arity (ty : Expr) : Nat := Id.run do
  let mut e := ty
  let mut n := 0
  repeat
    match e.consumeMData with
    | .forallE _ _ b _ => n := n + 1; e := b
    | _ => break
  return n

/-- Per tracked theorem: the argument seen so far at each explicit binder (`none` once they
differ), number of calls, and whether it escaped. -/
structure ArgInfo where
  args : Array (Option Expr)
  calls : Nat := 0
  escapes : Bool := false

/-- A proposition: a `∀` or an application of a constant whose type ends in `Prop`. -/
def looksProp (env : Environment) (t : Expr) : Bool :=
  match t.consumeMData with
  | .forallE .. => true
  | e =>
    match e.getAppFn with
    | .const c _ =>
      match env.find? c with
      | some ci => Id.run do
        let mut ty := ci.type
        repeat
          match ty with
          | .forallE _ _ b _ => ty := b
          | _ => break
        return ty.isProp
      | none => false
    | _ => false

def varHole : Name := `_flat_var

/-- Loose variables of a term (relative to its top), in order of first occurrence. -/
partial def looseOrder (e : Expr) (d : Nat) (acc : Array Nat) : Array Nat :=
  if !e.hasLooseBVars then acc else
  match e with
  | .bvar i => if i ≥ d && !acc.contains (i - d) then acc.push (i - d) else acc
  | .app f a => looseOrder a d (looseOrder f d acc)
  | .lam _ t b _ | .forallE _ t b _ => looseOrder b (d + 1) (looseOrder t d acc)
  | .letE _ t v b _ => looseOrder b (d + 1) (looseOrder v d (looseOrder t d acc))
  | .mdata _ b | .proj _ _ b => looseOrder b d acc
  | _ => acc

/-- Replace each loose variable by a placeholder that keeps which variable it is (in order of
first occurrence) and its binder's type when that type is closed. -/
def fingerprint (t : Expr) (types : List Expr) : Expr :=
  let ids := looseOrder t 0 #[]
  let subst := (List.range t.looseBVarRange).toArray.map fun i =>
    let ty := (types[i]?).getD (.sort .zero)
    mkApp2 (mkConst varHole) (mkRawNatLit ((ids.idxOf? i).getD 0))
      (if ty.hasLooseBVars then mkConst varHole else ty)
  t.instantiate subst

def trivialProof (v : Expr) : Bool :=
  v.consumeMData.isConst ||
    [``rfl, ``Eq.refl, ``trivial, `le_refl, `le_rfl, ``Iff.rfl, ``HEq.refl].any
      (v.consumeMData.getAppFn.isConstOf ·)

abbrev FactM := StateM (Array (Expr × Nat × Name) × Nat × Std.HashSet Expr)

/-- `have`-level facts of a term: (fingerprint, subproof size, bound name). At most 400,000
nodes per declaration; a closed subterm is visited once. -/
partial def facts (env : Environment) (minSize : Nat) (e : Expr) : FactM Unit :=
  go e []
where
  go (e : Expr) (types : List Expr) : FactM Unit := do
    let (acc, steps, seen) ← get
    if steps > 400000 then return
    if !e.hasLooseBVars && !e.isAtomic then
      if seen.contains e then return
      set (acc, steps + 1, seen.insert e)
    else set (acc, steps + 1, seen)
    let record (nm : Name) (t v : Expr) : FactM Unit := do
      if looksProp env t && !trivialProof v then
        let n := size v
        let fp := fingerprint t types
        if n ≥ minSize && size fp ≥ 2 * minSize then
          modify fun (a, s, h) => (a.push (fp, n, nm.eraseMacroScopes), s, h)
    match e with
    | .letE nm t v b _ => record nm t v; go t types; go v types; go b (t :: types)
    | .app .. =>
      let f := e.getAppFn
      let args := e.getAppArgs
      match f.consumeMData with
      | .lam nm t _ _ =>
        if let some v := args[0]? then record nm t v
      | .const ``letFun _ =>
        let nm : Name := match (args[3]? : Option Expr) with
          | some (Expr.lam n _ _ _) => n
          | _ => Name.anonymous
        if let (some t, some v) := (args[0]?, args[2]?) then record nm t v
      | _ => pure ()
      go f types
      for a in args do go a types
    | .lam _ t b _ | .forallE _ t b _ => go t types; go b (t :: types)
    | .mdata _ b | .proj _ _ b => go b types
    | _ => pure ()

def run (o : Opts) (imports roots : Array Name) : IO Unit := do
  let env ← importModules (imports.map ({ module := · })) {} (loadExts := true)
  let isProj ← projectModules env
  for r in roots do
    unless env.contains r do throw <| IO.userError s!"unknown root constant {r}"
  let reached := reachable env isProj roots
  let decls := (env.constants.map₁.toList.filter fun (n, _) =>
    inProject env isProj n && reached.contains n).toArray
  -- Tracked theorems: reached, user-written, explicit binders recorded.
  let mut info : Std.HashMap Name (ArgInfo × Array Nat) := {}
  for (n, ci) in decls do
    if ci matches .thmInfo _ && userWritten env n ci then
      let mut expl := #[]
      let mut ty := ci.type
      let mut i := 0
      repeat
        match ty.consumeMData with
        | .forallE _ _ b bi => if bi == .default then expl := expl.push i
                               ty := b; i := i + 1
        | _ => break
      info := info.insert n ({ args := expl.map fun _ => none }, expl)
  let mut callers : Std.HashMap Name NameSet := {}
  let mut sites : Std.HashMap (Name × Name) (Std.HashSet Expr) := {}
  let mut seenArg : Std.HashMap (Name × Nat) (Option Expr) := {}
  let mut escaped : NameSet := {}
  let mut calls : Std.HashMap Name Nat := {}
  let mut factsOf : Std.HashMap Expr (Array (Name × Nat × Name)) := {}
  for (n, ci) in decls do
    let own := owner env n
    let terms := #[ci.type] ++ (ci.value? (allowOpaque := true)).toArray
    for e in terms do
      -- References and applications, each distinct subterm once.
      let mut todo := #[e]
      let mut visited : Std.HashSet Expr := {}
      while !todo.isEmpty do
        let x := todo.back!
        todo := todo.pop
        if visited.contains x then continue
        visited := visited.insert x
        match x with
        | .const c _ =>
          if let some _ := info[c]? then
            if c != own then
              callers := callers.insert c ((callers.getD c {}).insert own)
            escaped := escaped.insert c
        | .app .. =>
          let f := x.getAppFn
          let args := x.getAppArgs
          if let .const c _ := f then
            if let some (_, expl) := info[c]? then
              if c != own then
                callers := callers.insert c ((callers.getD c {}).insert own)
              let full : Bool := match env.find? c with
                | some cci => decide (args.size ≥ arity cci.type)
                | none => false
              if full then
                sites := sites.insert (c, own) ((sites.getD (c, own) {}).insert x)
                calls := calls.insert c (calls.getD c 0 + 1)
                for j in [0:expl.size] do
                  let a := args[expl[j]!]!
                  let key := (c, j)
                  let v : Option Expr := if a.hasLooseBVars then none else some a
                  match seenArg[key]? with
                  | none => seenArg := seenArg.insert key v
                  | some (some prev) =>
                    if v != some prev then seenArg := seenArg.insert key none
                  | some none => pure ()
                for a in args do todo := todo.push a
                continue
              else escaped := escaped.insert c
            for a in args do todo := todo.push a
          else
            todo := todo.push f
            for a in args do todo := todo.push a
        | .lam _ t b _ | .forallE _ t b _ => todo := todo.push t |>.push b
        | .letE _ t v b _ => todo := todo.push t |>.push v |>.push b
        | .mdata _ b | .proj _ _ b => todo := todo.push b
        | _ => pure ()
    if let some v := ci.value? (allowOpaque := true) then
      let ((), (fs, _, _)) := (facts env o.minSize v).run (#[], 0, {})
      for (fp, sz, nm) in fs do
        let old := factsOf.getD fp #[]
        unless old.any (·.1 == own) do factsOf := factsOf.insert fp (old.push (own, sz, nm))
  let range (n : Name) : CoreM (Nat × Nat) := do
    match ← findDeclarationRanges? n with
    | some r => return (r.range.pos.line, r.range.endPos.line)
    | none => return (0, 0)
  let out ← IO.getStdout
  let act : MetaM Unit := do
    let mut nSingle := 0
    let names := info.toArray.map (·.1) |>.qsort (·.toString < ·.toString)
    for n in names do
      let cs := callers.getD n {}
      if cs.size != 1 then continue
      let c := cs.toList.head!
      let (a, b) ← range n
      if a == 0 then continue
      let k := (sites.getD (n, c) {}).size
      let esc := if escaped.contains n then 1 else 0
      out.putStrLn s!"single\t{moduleOf env n}\t{userName n}\t{a}\t{b}\t{userName c}\t\
        {moduleOf env c}\t{k}\t{esc}"
      nSingle := nSingle + 1
    let mut nConst := 0
    for n in names do
      if escaped.contains n then continue
      let ncalls := calls.getD n 0
      if ncalls < o.minCalls then continue
      let some (_, expl) := info[n]? | continue
      let some ci := env.find? n | continue
      let bs := Id.run do
        let mut out := #[]
        let mut ty := ci.type
        repeat
          match ty.consumeMData with
          | .forallE bn _ b _ => out := out.push bn; ty := b
          | _ => break
        return out
      for j in [0:expl.size] do
        let some (some arg) := seenArg[(n, j)]? | continue
        let (a, b) ← range n
        let txt ← try pure (toString (← ppExpr arg)) catch _ => pure (toString arg)
        let txt := " ".intercalate (txt.split (· == '\n') |>.toList.map (·.trimAscii.toString))
        let txt := if txt.length > 120 then (txt.take 120).toString ++ " …" else txt
        let bn := (bs[expl[j]!]?.getD .anonymous).eraseMacroScopes
        out.putStrLn s!"constarg\t{moduleOf env n}\t{userName n}\t{a}\t{b}\t{expl[j]!}\t{bn}\t\
          {ncalls}\t{txt}"
        nConst := nConst + 1
    -- Facts proved in several declarations.
    let groups := (factsOf.toArray.filter (·.2.size ≥ 2)).qsort fun x y =>
      x.2.foldl (· + ·.2.1) 0 > y.2.foldl (· + ·.2.1) 0
    let mut g := 0
    let mut nPairs := 0
    for (fp, members) in groups do
      g := g + 1
      let txt ← try pure (toString (← ppExpr fp)) catch _ => pure (toString fp)
      let txt := " ".intercalate (txt.split (· == '\n') |>.toList.map (·.trimAscii.toString))
      let txt := (txt.replace "_flat_var" "·")
      let txt := if txt.length > 160 then (txt.take 160).toString ++ " …" else txt
      for (d, sz, nm) in members do
        let (a, b) ← range d
        out.putStrLn s!"fact\t{g}\t{sz}\t{moduleOf env d}\t{userName d}\t{a}\t{b}\t{nm}\t{txt}"
      for (x, _) in members do
        for (y, _) in members do
          if x != y && ((callers.getD y {}).contains x) then
            out.putStrLn s!"factpair\t{g}\t{userName x}\t{userName y}"
            nPairs := nPairs + 1
    out.putStrLn s!"# {reached.size} project constants reached; {info.size} user-written theorems"
    out.putStrLn s!"# single-caller theorems: {nSingle}"
    out.putStrLn s!"# arguments constant at every call (at least {o.minCalls} calls): {nConst}"
    out.putStrLn s!"# facts proved in several declarations: {groups.size} groups, \
      {nPairs} caller/callee pairs"
  let _ ← act.toIO { fileName := "<flat>", fileMap := default, maxHeartbeats := 0 } { env }

end Flat

partial def parseArgs (o : Flat.Opts) : List String → Option (Flat.Opts × Array Name × Array Name)
  | "--min-size" :: k :: rest => k.toNat?.bind fun k => parseArgs { o with minSize := k } rest
  | "--min-calls" :: k :: rest => k.toNat?.bind fun k => parseArgs { o with minCalls := k } rest
  | args =>
    match args.span (· != "--") with
    | (mods, "--" :: roots) =>
      if mods.isEmpty || roots.isEmpty then none
      else some (o, mods.toArray.map String.toName, roots.toArray.map String.toName)
    | _ => none

unsafe def main (args : List String) : IO UInt32 := do
  let some (o, imports, roots) := parseArgs {} args
    | IO.eprintln "usage: lake env lean --run Flat.lean [--min-size n] [--min-calls n] \
        <module>... -- <root constant>..."
      return 2
  initSearchPath (← findSysroot)
  enableInitializersExecution
  try
    Flat.run o imports roots
    return 0
  catch e =>
    IO.eprintln s!"flat: {e}"
    return 1
