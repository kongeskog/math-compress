/-
Weight: how much of the proof each declaration alone holds up (dominators).

Usage, from inside the Lake project directory (the one with `lakefile.toml`):

    lake env lean --run Weight.lean [--check NAME]… <module>… -- <root>…

Options first; then, as for `Reach.lean`, the modules to import before `--` and the root
constants after it. The graph has one node per user-written declaration of the project that the
roots reach; generated declarations (`foo._proof_1`, `foo.match_1`, ...) are merged into the
declaration they belong to, and an edge goes from a declaration to every one whose constants
occur in its types and values. A virtual root points to the root constants. A declaration D
dominates E when every path from the virtual root to E passes through D: removing or replacing
D makes E unreachable. Immediate dominators are computed with the iterative algorithm of
Cooper, Harvey and Kennedy over a reverse postorder.

Output, one record per line, fields separated by tabs:

    node  <module> <name> <first> <last> <immediate dominator or -> <depth>
    check <name> <dominated> <unreachable without it> <same>

`depth` is the depth in the dominator tree (root constants and everything shared by several
of them hang under the virtual root, depth 1). The weights (source tokens of everything a node
dominates) are computed from the ranges by `hc analyze weight`. `--check NAME` recomputes
reachability with NAME removed and compares the declarations that become unreachable with those
NAME dominates; they must be the same set. Lines starting with `# ` are a summary.

Caveats. References that leave no trace in terms (simp lemmas proved by `rfl`, instances tried
during elaboration) are missing, as in `Reach.lean`. Dominance says what the final theorems'
terms need, not what a different proof could avoid.
-/
import Lean

open Lean Meta

namespace Weight

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

/-- Nodes reachable from the virtual root (index 0), skipping `banned`. -/
def reach (succ : Array (Array Nat)) (banned : Option Nat) : Array Bool := Id.run do
  let mut seen := Array.replicate succ.size false
  let mut stack := #[0]
  while !stack.isEmpty do
    let v := stack.back!
    stack := stack.pop
    if seen[v]! || some v == banned then continue
    seen := seen.set! v true
    for w in succ[v]! do
      if !seen[w]! then stack := stack.push w
  return seen

def run (checks : Array Name) (imports roots : Array Name) : IO Unit := do
  let env ← importModules (imports.map ({ module := · })) {} (loadExts := true)
  let isProj ← projectModules env
  for r in roots do
    unless env.contains r do throw <| IO.userError s!"unknown root constant {r}"
  let reached := reachable env isProj roots
  -- Nodes: index 0 is the virtual root.
  let mut idx : Std.HashMap Name Nat := {}
  let mut names : Array Name := #[.anonymous]
  let mut edges : Std.HashMap Nat (Std.HashSet Nat) := {}
  let consts := env.constants.map₁.toList.filter fun (n, _) =>
    inProject env isProj n && reached.contains n
  for (n, _) in consts do
    let o := owner env n
    unless idx.contains o do
      idx := idx.insert o names.size
      names := names.push o
  for (n, ci) in consts do
    let a := idx[owner env n]!
    let mut deps := ci.type.getUsedConstants
    if let some v := ci.value? (allowOpaque := true) then deps := deps ++ v.getUsedConstants
    match ci with
    | .inductInfo i => deps := deps ++ i.ctors.toArray
    | _ => pure ()
    for d in deps do
      if let some b := idx[owner env d]? then
        if a != b then edges := edges.insert a ((edges.getD a {}).insert b)
  let n := names.size
  let mut succ : Array (Array Nat) := Array.replicate n #[]
  for (a, bs) in edges.toList do
    succ := succ.set! a (bs.toArray.qsort (· < ·))
  succ := succ.set! 0 (roots.filterMap (idx[·]?))
  -- Reverse postorder from the virtual root.
  let mut order : Array Nat := #[]
  let mut visited := Array.replicate n false
  let mut stack : Array (Nat × Nat) := #[(0, 0)]
  visited := visited.set! 0 true
  while !stack.isEmpty do
    let (v, i) := stack.back!
    if i < succ[v]!.size then
      stack := stack.set! (stack.size - 1) (v, i + 1)
      let w := succ[v]![i]!
      if !visited[w]! then
        visited := visited.set! w true
        stack := stack.push (w, 0)
    else
      order := order.push v
      stack := stack.pop
  order := order.reverse
  let mut rpo := Array.replicate n 0
  for h : i in [0:order.size] do rpo := rpo.set! order[i] i
  let mut preds : Array (Array Nat) := Array.replicate n #[]
  for v in order do
    for w in succ[v]! do preds := preds.set! w (preds[w]!.push v)
  let none' := n
  let mut idom := Array.replicate n none'
  idom := idom.set! 0 0
  let mut changed := true
  while changed do
    changed := false
    for v in order do
      if v == 0 then continue
      let mut new := none'
      for p in preds[v]! do
        if idom[p]! == none' then continue
        if new == none' then new := p
        else
          -- intersect
          let mut a := p
          let mut b := new
          while a != b do
            while rpo[a]! > rpo[b]! do a := idom[a]!
            while rpo[b]! > rpo[a]! do b := idom[b]!
          new := a
      if new != none' && idom[v]! != new then
        idom := idom.set! v new
        changed := true
  -- Depths.
  let mut depth := Array.replicate n 0
  for v in order do
    if v != 0 then depth := depth.set! v (depth[idom[v]!]! + 1)
  let range (m : Name) : CoreM (Nat × Nat) := do
    match ← findDeclarationRanges? m with
    | some r => return (r.range.pos.line, r.range.endPos.line)
    | none => return (0, 0)
  let out ← IO.getStdout
  let act : CoreM Unit := do
    for v in order do
      if v == 0 then continue
      let m := names[v]!
      let (a, b) ← range m
      let d := if idom[v]! == 0 then "-" else toString (userName names[idom[v]!]!)
      out.putStrLn s!"node\t{moduleOf env m}\t{userName m}\t{a}\t{b}\t{d}\t{depth[v]!}"
  let _ ← act.toIO { fileName := "<weight>", fileMap := default, maxHeartbeats := 0 } { env }
  -- Checks: dominated set against what becomes unreachable without the node.
  let all := reach succ none
  for c in checks do
    let some v := idx[c]? <|> names.findIdx? (userName · == c)
      | out.putStrLn s!"check\t{c}\tunknown\t-\t-"; continue
    let without := reach succ (some v)
    let mut lost : Array Nat := #[]
    for w in [1:n] do
      if all[w]! && !without[w]! then lost := lost.push w
    let mut dom : Array Nat := #[]
    for w in [1:n] do
      let mut x := w
      while x != 0 && x != v do x := idom[x]!
      if x == v then dom := dom.push w
    let same := lost == dom
    out.putStrLn s!"check\t{c}\t{dom.size}\t{lost.size}\t{if same then "yes" else "no"}"
  out.putStrLn s!"# {n - 1} declarations reached (generated ones merged into their owners)"
  out.putStrLn s!"# dominator tree depth: {depth.foldl max 0}"

end Weight

partial def parseArgs (cs : Array Name) :
    List String → Option (Array Name × Array Name × Array Name)
  | "--check" :: c :: rest => parseArgs (cs.push c.toName) rest
  | args =>
    match args.span (· != "--") with
    | (mods, "--" :: roots) =>
      if mods.isEmpty || roots.isEmpty then none
      else some (cs, mods.toArray.map String.toName, roots.toArray.map String.toName)
    | _ => none

unsafe def main (args : List String) : IO UInt32 := do
  let some (cs, imports, roots) := parseArgs #[] args
    | IO.eprintln "usage: lake env lean --run Weight.lean [--check NAME]... <module>... -- \
        <root constant>..."
      return 2
  initSearchPath (← findSysroot)
  enableInitializersExecution
  try
    Weight.run cs imports roots
    return 0
  catch e =>
    IO.eprintln s!"weight: {e}"
    return 1
