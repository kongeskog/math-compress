/-
Seams: what a lemma boundary hides, seen from both sides of it.

Usage, from inside the Lake project directory (the one with `lakefile.toml`):

    lake env lean --run Seams.lean [--min-size n] [--max-diff k] <module>… -- <root>…

Options first; then, as for `Reach.lean`, the modules to import before `--` and the root
constants after it. Only the project's own declarations that the roots reach count. A
single-caller lemma is a user-written theorem referenced by exactly one other user-written
declaration (generated declarations such as `foo._proof_1` count as part of their owner).
Records, one per line, fields separated by tabs:

    part     <module> <lemma> <first> <last> <caller> <read>/<pieces> <unread>/<nodes> <unread>
    undo     <module> <lemma> <first> <last> <caller> <eliminated|result|argument> <lemma>
    thread   <module> <decl> <first> <last> <i> <binder> <used|dead> <carriers> <names>
    skel     <group> <nodes> <module> <decl> <first> <last> <have> <skeleton> <difference>
    dispatch <module> <lemma> <first> <last> <caller> <branch lemmas>

`part`: the pieces of a single-caller lemma's conclusion (conjuncts, and the witnesses and body
of each `∃`, as in `Unused.lean`, after `∀`/`→` and through the project's own definitions) that
its one caller reads, followed through `.1`/`.2`, `obtain`, matchers, `have`s and beta-redexes.
A witness counts as read when its value is used, not when it only occurs in the type of the
property bound with it. Anything not understood counts as reading everything. When the proof is
an anonymous constructor at the top (`⟨a, b⟩`, `Exists.intro`), the unread pieces' share of the
proof term is given in nodes; otherwise `-`.

`undo`: work done on one side of the boundary and undone on the other. `eliminated`: the caller
applies to the lemma's result (or to one conjunct of it) a lemma of the namespace of that
statement's head (`Filter.Tendsto.eventually` to a `Filter.Tendsto`, `abs_le…` is not caught):
the lemma states a general form its caller takes apart at once. `result`: the caller applies
`foo.1` (or `.2`) to the result and the proof ends with `foo.2` (or `.1`) of the same `Iff`
lemma. `argument`: the caller passes `foo.2 h` (or `.1`) as an argument and the lemma uses that
parameter only through `foo.1` (or `.2`).

`thread`: a hypothesis (a binder whose type is a proposition) that declarations only pass on,
unchanged, to other declarations' binders. `used`: the record names the declaration that really
uses it (other than by passing it on) and the user-written declarations that only carry it
there (at least two); `dead`: a declaration passes it on but no declaration on the chain uses
it (the record names where it is passed).

`skel`: subproofs, at declaration level and at `have` level (also inside one declaration),
whose eliminator skeletons are the same (the sequence of case splits: `casesOn`/`rec` of an
inductive type, matchers by their number of alternatives, `Or.elim`, `lt_trichotomy`,
`le_or_lt`, `dite`, `Decidable.byCases`, ... in prefix order, at least two) and whose
statements are equal or differ in at most `--max-diff` constants (default 2), with at least
`--min-size` nodes (default 40) of proof. Members of a group share its number.

`dispatch`: a single-caller lemma whose proof is a case split in which every branch is one
application of another lemma.

Caveats. Terms show what the elaborated proofs contain, not the text: a pattern that the text
shows but the term does not (for example a restated type annotation) is not seen here; `hc
analyze seams` adds source-level checks for those. Everything is a candidate: change, build,
compare.
-/
import Lean

open Lean Meta

namespace Seams

/-! ### Project, reachability, generated names (as in `Reach.lean`) -/

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

/-! ### Splitting statements into conjuncts -/

inductive Step where
  | idx (i : Nat)
  | ex
  | wit
  | app
  deriving BEq, Hashable, Inhabited

def Step.toString : Step → String
  | .idx i => s!"{i}"
  | .ex => "E"
  | .wit => "W"
  | .app => "A"

/-- Unfold an application of one of the project's definitions, if it is one. -/
def unfoldProject? (env : Environment) (isProj : Array Bool) (e : Expr) : Option Expr :=
  match e.getAppFn with
  | .const c lvls =>
    if !inProject env isProj c then none else
    match env.find? c with
    | some ci@(.defnInfo _) =>
      if isMatcherCore env c then none
      else some ((ci.instantiateValueLevelParams! lvls).beta e.getAppArgs)
    | _ => none
  | _ => none

/-- The pieces of a statement, as paths of steps. Pure version, used by the analysis. -/
partial def leaves (env : Environment) (isProj : Array Bool) (fuel : Nat) (e : Expr) :
    Array (List Step) :=
  let e := e.consumeMData
  match e with
  | .forallE _ _ b _ => (leaves env isProj fuel b).map (.app :: ·)
  | _ =>
    if e.isAppOfArity ``And 2 then
      (leaves env isProj fuel e.appFn!.appArg!).map (.idx 0 :: ·) ++
        (leaves env isProj fuel e.appArg!).map (.idx 1 :: ·)
    else if e.isAppOfArity ``Exists 2 then
      match e.appArg!.consumeMData with
      | .lam _ _ b _ => #[[Step.wit]] ++ (leaves env isProj fuel b).map (.ex :: ·)
      | _ => #[[]]
    else if fuel > 0 then
      match unfoldProject? env isProj e with
      | some e' => leaves env isProj (fuel - 1) e'
      | none => #[[]]
    else #[[]]

def oneLine (s : String) (max : Nat := 160) : String :=
  let s := " ".intercalate ((s.split (fun c => c == '\n' || c == ' ')).toList.filterMap fun w =>
    if w.isEmpty then none else some w.toString)
  let s := s.replace "\t" " "
  if s.length > max then (s.take max).toString ++ " …" else s

/-- The same splitting in `MetaM`, with each piece pretty-printed in its context. -/
partial def leavesPP (isProj : Array Bool) (fuel : Nat) (e : Expr) :
    MetaM (Array (List Step × String)) := do
  let env ← getEnv
  let e := e.consumeMData
  match e with
  | .forallE n t b bi =>
    withLocalDecl n bi t fun x => do
      return (← leavesPP isProj fuel (b.instantiate1 x)).map fun (p, s) => (.app :: p, s)
  | _ =>
    if e.isAppOfArity ``And 2 then
      let l ← leavesPP isProj fuel e.appFn!.appArg!
      let r ← leavesPP isProj fuel e.appArg!
      return l.map (fun (p, s) => (.idx 0 :: p, s)) ++ r.map (fun (p, s) => (.idx 1 :: p, s))
    else if e.isAppOfArity ``Exists 2 then
      match e.appArg!.consumeMData with
      | .lam n t b bi =>
        let w := ([Step.wit], s!"witness {n.eraseMacroScopes} : {← pp t}")
        withLocalDecl n bi t fun x => do
          return #[w] ++
            (← leavesPP isProj fuel (b.instantiate1 x)).map fun (p, s) => (.ex :: p, s)
      | _ => return #[([], ← pp e)]
    else if fuel > 0 then
      match unfoldProject? env isProj e with
      | some e' => leavesPP isProj (fuel - 1) e'
      | none => return #[([], ← pp e)]
    else return #[([], ← pp e)]
where
  pp (e : Expr) : MetaM String := do
    try return oneLine (toString (← ppExpr e))
    catch _ => return oneLine (toString e)

def unfoldFuel : Nat := 6

def isPrefix : List Step → List Step → Bool
  | [], _ => true
  | _, [] => false
  | a :: as, b :: bs => a == b && isPrefix as bs

/-- A piece is read when some read path is a prefix of it or extends it. -/
def leafRead (marks : Array (List Step)) (leaf : List Step) : Bool :=
  marks.any fun m => isPrefix m leaf || isPrefix leaf m

/-! ### The walk -/

inductive Src where
  | thm (n : Name)
  | hyp (d : Name) (i : Nat)
  deriving BEq, Hashable, Inhabited

structure Origin where
  src : Src
  /-- Steps taken so far, last first. -/
  rpath : List Step
  deriving Inhabited

/-- What the analysis knows about a value. -/
inductive Val where
  | none
  /-- The value of field `i` of a project structure. -/
  | field (s : Name) (i : Nat)
  /-- A proof of a part of a tracked statement. -/
  | path (os : Array Origin)
  deriving Inhabited

inductive Ent where
  | val (v : Val)
  | thunk (id : Nat)
  deriving Inhabited

/-- An environment for the loose variables of a term, with an identity for memoization. -/
structure Env where
  ents : List Ent := []
  id : Nat := 0
  deriving Inhabited

structure Thunk where
  e : Expr
  env : Env
  memo : Option Val := none
  deriving Inhabited

structure Ctx where
  env : Environment
  isProj : Array Bool
  /-- Project structures and the number of their fields. -/
  structs : Std.HashMap Name Nat
  /-- Generated constants of a project structure; any occurrence reads every field. -/
  allOf : Std.HashMap Name Name
  /-- Theorems with at least two conjuncts. -/
  tracked : NameSet
  budget : Nat
  progress : Bool

structure St where
  thunks : Array Thunk := #[]
  fields : Std.HashSet (Name × Nat) := {}
  marks : Std.HashMap Src (Array (List Step)) := {}
  uses : Std.HashMap Name Nat := {}
  closed : Std.HashMap Expr Val := {}
  /-- Results for terms with loose variables, by environment; cleared per declaration. -/
  memo : Std.HashMap (Expr × Nat) Val := {}
  /-- The environment extended by an untracked variable, by environment. -/
  noneChild : Std.HashMap Nat Nat := {}
  nextEnv : Nat := 1
  steps : Nat := 0

abbrev W := ReaderT Ctx (StateRefT St IO)

def budgetMsg : String := "seams: step budget exceeded"

@[inline] def tick : W Unit := do
  let budget := (← read).budget
  let n ← modifyGet fun s => (s.steps, { s with steps := s.steps + 1 })
  if n ≥ budget then throw (IO.userError budgetMsg)

def markField (s : Name) (i : Nat) : W Unit :=
  modify fun st => { st with fields := st.fields.insert (s, i) }

def markAllFields (s : Name) : W Unit := do
  let n := (← read).structs.getD s 0
  for i in [0:n] do markField s i

def markPath (src : Src) (p : List Step) : W Unit :=
  modify fun st =>
    let old := st.marks.getD src #[]
    if old.any (isPrefix · p) then st
    else { st with marks := st.marks.insert src (old.push p) }

def useWhole : Val → W Unit
  | .none => pure ()
  | .field s i => markField s i
  | .path os => for o in os do markPath o.src o.rpath.reverse

/-- Extend a tracked value by a step; anything else is read whole and yields nothing. -/
def extend (v : Val) (st : Step) : W Val := do
  match v with
  | .path os => return .path (os.map fun o => { o with rpath := st :: o.rpath })
  | v => useWhole v; return .none

-- The state is only changed through `modify`/`modifyGet`, so that its arrays and maps stay
-- unshared and are updated in place.

def push (ρ : Env) (x : Ent) : W Env := do
  let id ← modifyGet fun s => (s.nextEnv, { s with nextEnv := s.nextEnv + 1 })
  return { ents := x :: ρ.ents, id }

/-- Extending by an untracked variable is deterministic, so it reuses the identity. -/
def pushNone (ρ : Env) : W Env := do
  let id ← modifyGet fun s =>
    match s.noneChild[ρ.id]? with
    | some id => (id, s)
    | none =>
      let id := s.nextEnv
      (id, { s with nextEnv := id + 1, noneChild := s.noneChild.insert ρ.id id })
  return { ents := .val .none :: ρ.ents, id }

def newThunk (e : Expr) (ρ : Env) : W Nat :=
  modifyGet fun s => (s.thunks.size, { s with thunks := s.thunks.push { e, env := ρ } })

def constOcc (c : Name) : W Unit := do
  let ctx ← read
  if let some info := ctx.env.getProjectionFnInfo? c then
    if let some (.ctorInfo ci) := ctx.env.find? info.ctorName then
      if ctx.structs.contains ci.induct then markField ci.induct info.i
  if let some s := ctx.allOf[c]? then markAllFields s

inductive RecKind where
  | viaCasesOn
  | viaRecOn
  | viaRec

/-- Recognize `casesOn`, `recOn` and `rec` of a non-recursive, non-mutual inductive type. -/
def recShape? (env : Environment) (c : Name) : Option (InductiveVal × RecKind) := do
  let (ind, k) ←
    if isCasesOnRecursor env c then some (c.getPrefix, RecKind.viaCasesOn)
    else if isAuxRecursorWithSuffix env c "recOn" then some (c.getPrefix, RecKind.viaRecOn)
    else match env.find? c with
      | some (.recInfo r) => if r.numMotives == 1 then some (c.getPrefix, RecKind.viaRec) else none
      | _ => none
  match env.find? ind with
  | some (.inductInfo i) =>
    if i.isRec || i.all.length != 1 || i.isNested then none else some (i, k)
  | _ => none

mutual

partial def eval (e : Expr) (ρ : Env) : W Val := do
  tick
  match e with
  | .app .. | .lam .. | .forallE .. | .letE .. | .proj .. | .mdata .. =>
    if e.hasLooseBVars then
      let key := (e, ρ.id)
      if let some v := (← get).memo[key]? then return v
      let v ← evalCore e ρ
      modify fun s => { s with memo := s.memo.insert key v }
      return v
    else
      if let some v := (← get).closed[e]? then return v
      let v ← evalCore e {}
      modify fun s => { s with closed := s.closed.insert e v }
      return v
  | _ => evalCore e ρ

partial def use (e : Expr) (ρ : Env) : W Unit := do
  useWhole (← eval e ρ)

partial def lookup (ρ : Env) (i : Nat) : W Val := do
  match ρ.ents[i]? with
  | some (.val v) => return v
  | some (.thunk id) => force id
  | none => return .none

partial def force (id : Nat) : W Val := do
  let t := (← get).thunks[id]!
  if let some v := t.memo then return v
  let v ← eval t.e t.env
  modify fun s => { s with thunks := s.thunks.modify id ({ · with memo := some v }) }
  return v

partial def evalCore (e : Expr) (ρ : Env) : W Val := do
  match e with
  | .bvar i => lookup ρ i
  | .mdata _ b => eval b ρ
  | .lam _ t b _ | .forallE _ t b _ =>
    use t ρ
    use b (← pushNone ρ)
    return .none
  | .letE _ t v b _ =>
    use t ρ
    let id ← newThunk v ρ
    eval b (← push ρ (.thunk id))
  | .proj s i x => projVal s i (← eval x ρ)
  | .app .. => evalApp e ρ
  | .const c _ =>
    constOcc c
    if (← read).tracked.contains c then
      modify fun s => { s with uses := s.uses.insert c (s.uses.getD c 0 + 1) }
      return .path #[{ src := .thm c, rpath := [] }]
    return .none
  | _ => return .none

partial def projVal (s : Name) (i : Nat) (v : Val) : W Val := do
  if s == ``And then return ← extend v (.idx i)
  if (← read).structs.contains s then markField s i
  useWhole v
  return .none

/-- A value applied to arguments: the arguments are read whole. -/
partial def appVal (v : Val) (args : Array Expr) (ρ : Env) : W Val := do
  for a in args do use a ρ
  let mut v := v
  for _ in args do v ← extend v .app
  return v

/-- The lambda behind a variable bound lazily, with its environment. -/
partial def lamOf (id : Nat) (fuel : Nat := 16) : W (Option (Expr × Env)) := do
  let t := (← get).thunks[id]!
  match t.e.consumeMData with
  | e@(.lam ..) => return some (e, t.env)
  | .bvar j =>
    match t.env.ents[j]? with
    | some (.thunk id') => if fuel == 0 then return none else lamOf id' (fuel - 1)
    | _ => return none
  | _ => return none

/-- Beta-reduce lazily: each argument is bound to a thunk and only read where used. -/
partial def applyLam (lam : Expr) (lρ : Env) (args : Array Expr) (ρ : Env) : W Val := do
  let mut body := lam
  let mut env := lρ
  let mut k := 0
  while k < args.size do
    match body with
    | .lam _ t b _ =>
      let _ := t  -- binder types of beta-bound lambdas are not read (see the header)
      let id ← newThunk args[k]! ρ
      env ← push env (.thunk id)
      body := b
      k := k + 1
    | .mdata _ b => body := b
    | _ => break
  let v ← eval body env
  if k < args.size then appVal v (args.extract k args.size) ρ else return v

/-- The value of a minor premise whose `nf` field variables carry `vals`. -/
partial def bindMinor (minor : Expr) (vals : Array Val) (ρ : Env) : W Val := do
  let mut body := minor
  let mut env := ρ
  let mut k := 0
  while k < vals.size do
    match body with
    | .lam _ t b _ =>
      let _ := t
      env ← push env (.val vals[k]!)
      body := b
      k := k + 1
    | .mdata _ b => body := b
    | _ => break
  if k < vals.size then
    for v in vals.extract k vals.size do useWhole v
    use body env
    return .none
  eval body env

partial def evalRec (ind : InductiveVal) (kind : RecKind) (args : Array Expr) (ρ : Env) :
    W (Option Val) := do
  let ctx ← read
  let p := ind.numParams
  let k := ind.numIndices
  let n := ind.ctors.length
  if args.size < p + 2 + k + n then return none
  let (majorPos, minorPos) := match kind with
    | .viaRec => (p + 1 + n + k, p + 1)
    | _ => (p + 1 + k, p + 2 + k)
  let stop := p + 2 + k + n
  for j in [0:stop] do
    unless j == majorPos || (minorPos ≤ j && j < minorPos + n) do use args[j]! ρ
  let v ← eval args[majorPos]! ρ
  let indName := ind.name
  let isAnd := indName == ``And
  let isEx := indName == ``Exists
  match v with
  | .path _ => if !(isAnd || isEx) then useWhole v
  | _ => useWhole v
  let mut results := #[]
  for j in [0:n] do
    let ctor := ind.ctors[j]!
    let nf := match ctx.env.find? ctor with
      | some (.ctorInfo c) => c.numFields
      | _ => 0
    let mut vals := #[]
    for f in [0:nf] do
      let fv ← if isAnd then (match v with
          | .path _ => extend v (.idx f)
          | _ => pure .none)
        else if isEx then (match v with
          | .path _ => if f == 1 then extend v .ex else extend v .wit
          | _ => pure .none)
        else if ctx.structs.contains indName then pure (.field indName f)
        else pure .none
      vals := vals.push fv
    results := results.push (← bindMinor args[minorPos + j]! vals ρ)
  let r ← if results.size == 1 then pure results[0]! else do
    for r in results do useWhole r
    pure .none
  return some (← appVal r (args.extract stop args.size) ρ)

partial def evalApp (e : Expr) (ρ : Env) : W Val := do
  let f := e.getAppFn
  let args := e.getAppArgs
  match f with
  | .lam .. => applyLam f ρ args ρ
  | .bvar i =>
    match ρ.ents[i]? with
    | some (.thunk id) =>
      match ← lamOf id with
      | some (lam, lρ) => applyLam lam lρ args ρ
      | none => appVal (← lookup ρ i) args ρ
    | _ => appVal (← lookup ρ i) args ρ
  | .const c lvls => evalConstApp c lvls args ρ
  | _ => appVal (← eval f ρ) args ρ

partial def evalConstApp (c : Name) (lvls : List Level) (args : Array Expr) (ρ : Env) :
    W Val := do
  let ctx ← read
  let env := ctx.env
  if ctx.tracked.contains c then
    return ← appVal (← evalCore (.const c lvls) ρ) args ρ
  if let some info := getMatcherInfoCore? env c then
    if args.size ≥ info.arity then
      if let some ci := env.find? c then
        return ← applyLam (ci.instantiateValueLevelParams! lvls) {} args ρ
  if c == ``letFun && args.size ≥ 4 then
    for a in args.extract 0 2 do use a ρ
    let v ← match args[3]!.consumeMData with
      | lam@(.lam ..) => applyLam lam ρ #[args[2]!] ρ
      | _ => appVal (← eval args[3]! ρ) #[args[2]!] ρ
    return ← appVal v (args.extract 4 args.size) ρ
  if c == ``Exists.elim && args.size ≥ 5 then
    for a in args.extract 0 3 do use a ρ
    let v ← eval args[3]! ρ
    let hv ← match v with
      | .path _ => extend v .ex
      | _ => do useWhole v; pure .none
    let wv ← match v with
      | .path _ => extend v .wit
      | _ => pure .none
    let r ← bindMinor args[4]! #[wv, hv] ρ
    return ← appVal r (args.extract 5 args.size) ρ
  if let some (ind, kind) := recShape? env c then
    if let some v ← evalRec ind kind args ρ then return v
  if let some info := env.getProjectionFnInfo? c then
    if args.size > info.numParams then
      if let some (.ctorInfo ci) := env.find? info.ctorName then
        for a in args.extract 0 info.numParams do use a ρ
        let v ← projVal ci.induct info.i (← eval args[info.numParams]! ρ)
        return ← appVal v (args.extract (info.numParams + 1) args.size) ρ
  appVal (← evalCore (.const c lvls) ρ) args ρ

end

/-! ### Declarations -/

/-- Values the walk does not read on their own: they are unfolded where they are used. -/
def skipValue (ctx : Ctx) (n : Name) : Bool :=
  let env := ctx.env
  isMatcherCore env n || env.isProjectionFn n || ctx.allOf.contains n || isAuxRecursor env n
    || isNoConfusion env n

/-- Binders of a type, syntactically: name, type (with loose variables), info. -/
def binders (ty : Expr) : Array (Name × Expr × BinderInfo) := Id.run do
  let mut out := #[]
  let mut e := ty
  repeat
    match e.consumeMData with
    | .forallE n t b bi => out := out.push (n, t, bi); e := b
    | _ => break
  return out

/-- Number of leading `fun` binders. -/
def lambdaCount (e : Expr) : Nat := Id.run do
  let mut e := e
  let mut n := 0
  repeat
    match e.consumeMData with
    | .lam _ _ b _ => n := n + 1; e := b
    | _ => break
  return n

def isTrackedHyp (env : Environment) (isProj : Array Bool) (bi : BinderInfo) (t : Expr) : Bool :=
  !bi.isInstImplicit && (leaves env isProj unfoldFuel t).size ≥ 2

/-- Walk one declaration: its type, and its value with tracked hypotheses bound. -/
def walkDecl (n : Name) (ci : ConstantInfo) (hyps : Bool) : W Unit := do
  let ctx ← read
  modify fun s => { s with thunks := #[], steps := 0, memo := {} }
  use ci.type {}
  if skipValue ctx n then return
  let some v := ci.value? (allowOpaque := true) | return
  let bs := binders ci.type
  let mut body := v
  let mut env : Env := {}
  let mut i := 0
  repeat
    match body with
    | .lam _ t b _ =>
      use t env
      let tracked := hyps && i < bs.size &&
        isTrackedHyp ctx.env ctx.isProj bs[i]!.2.2 bs[i]!.2.1
      let val : Val := if tracked then .path #[{ src := .hyp n i, rpath := [] }] else .none
      env ← push env (.val val)
      body := b
      i := i + 1
    | .mdata _ b => body := b
    | _ => break
  use body env

/-- When a declaration is too large: everything it mentions counts as read. -/
def fallback (n : Name) (ci : ConstantInfo) : W Unit := do
  let ctx ← read
  let env := ctx.env
  let mut todo := #[ci.type] ++ (ci.value? (allowOpaque := true)).toArray
  let mut seenMatchers : NameSet := {}
  let mut k := 0
  while k < todo.size do
    let e := todo[k]!
    k := k + 1
    for c in e.getUsedConstants do
      constOcc c
      if ctx.tracked.contains c then markPath (.thm c) []
      if isMatcherCore env c && !seenMatchers.contains c then
        seenMatchers := seenMatchers.insert c
        if let some v := (env.find? c).bind (·.value?) then todo := todo.push v
      if let some (ind, _) := recShape? env c then
        if ctx.structs.contains ind.name then markAllFields ind.name
    e.forEachWhere Expr.isProj fun x => do
      if let .proj s i _ := x then
        if ctx.structs.contains s then markField s i
  for i in [0:(binders ci.type).size] do
    markPath (.hyp n i) []

/-- Walk every reached declaration; returns those that exceeded the budget. -/
def walkAll (decls : Array (Name × ConstantInfo)) (rootSet : NameSet) : W (Array Name) := do
  let env := (← read).env
  let mut incomplete := #[]
  let mut count := 0
  let mut total := 0
  let start ← IO.monoMsNow
  for (n, ci) in decls do
    count := count + 1
    if (← read).progress && count % 500 == 0 then
      IO.eprintln s!"unused: {count}/{decls.size} declarations, {total} steps, \
        {(← IO.monoMsNow) - start} ms"
    let hyps := userWritten env n ci && !rootSet.contains n && !isInstanceCore env n
    try
      walkDecl n ci hyps
    catch ex =>
      if toString ex == budgetMsg then
        incomplete := incomplete.push n
        fallback n ci
      else throw ex
    total := total + (← get).steps
    -- A hypothesis that occurs in the rest of the type is read whole.
    let mut tail := ci.type
    for i in [0:(binders ci.type).size] do
      match tail.consumeMData with
      | .forallE _ _ b _ =>
        if b.hasLooseBVar 0 then markPath (.hyp n i) []
        tail := b
      | _ => break
  return incomplete

/-! ### Helpers from `Flat.lean` and `Twins.lean` -/

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

partial def looksProp (env : Environment) (t : Expr) : Bool :=
  match t.consumeMData with
  | .forallE _ _ b _ => looksProp env b
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


/-- As `diffs`, but a pair of differing subterms (of at most twelve nodes each) counts as one
difference. -/
partial def subtreeDiffs (a b : Expr) (k : Nat) : Option (Array (Expr × Expr)) :=
  match go a b (#[], 0) with
  | some (out, _) => some out
  | none => none
where
  go (a b : Expr) (st : Array (Expr × Expr) × Nat) : Option (Array (Expr × Expr) × Nat) := do
    let (out, steps) := st
    if steps > 200000 then none
    let st := (out, steps + 1)
    let differ : Option (Array (Expr × Expr) × Nat) :=
      if a == b then some st
      else if out.any (fun (p, q) => p == a && q == b) then some st
      else if out.size ≥ k || size a 16 > 12 || size b 16 > 12 then none
      else some (out.push (a, b), steps + 1)
    match a, b with
    | .app f x, .app g y =>
      if a.getAppNumArgs == b.getAppNumArgs && a.getAppFn.isConst == b.getAppFn.isConst then
        go x y (← go f g st)
      else differ
    | .lam _ t c _, .lam _ u d _ | .forallE _ t c _, .forallE _ u d _ => go c d (← go t u st)
    | .mdata _ x, _ => go x b st
    | _, .mdata _ y => go a y st
    | .const c _, .const d _ => if c == d then some st else differ
    | _, _ => differ

/-- The conclusion of a statement after its leading binders. -/
def conclusion (e : Expr) : Expr := Id.run do
  let mut e := e
  repeat
    match e with
    | .forallE _ _ b _ | .mdata _ b => e := b
    | _ => break
  return e

/-- A difference as text, short. -/
def diffStr (e : Expr) : String :=
  let s := match e with
    | .const c _ => toString (userName c)
    | .lit (.natVal n) => toString n
    | _ => match e.getAppFn with
      | .const c _ => s!"{userName c} …"
      | _ => "(term)"
  s

/-- Statements that package an estimate (limits, asymptotics): a caller that takes one apart at
once with a lemma of its namespace may only need the estimate. -/
def packagingHeads : List Name :=
  [`Filter.Tendsto, `Asymptotics.IsLittleO, `Asymptotics.IsBigO, `Asymptotics.IsEquivalent,
   `Filter.Eventually, `Summable, `HasSum, `Filter.Frequently]

/-- Heads too common to say anything when a lemma of their namespace is applied. -/
def basicHeads : List Name :=
  [``Eq, ``LE.le, ``LT.lt, ``Ne, ``And, ``Or, ``Iff, ``Not, ``Exists, ``GE.ge, ``GT.gt,
   ``HEq, ``True, ``False, ``Membership.mem]

/-- `(x.1).2`-style projections of a conjunction: the base term and the path into it. -/
partial def stripAnd (e : Expr) : Expr × List Nat :=
  let e := e.consumeMData
  if e.isAppOfArity ``And.left 3 then
    let (b, p) := stripAnd e.appArg!
    (b, p ++ [0])
  else if e.isAppOfArity ``And.right 3 then
    let (b, p) := stripAnd e.appArg!
    (b, p ++ [1])
  else match e with
    | .proj ``And i x =>
      let (b, p) := stripAnd x
      (b, p ++ [i])
    | _ => (e, [])

/-- The part of a conjunction at a path of `.1`/`.2` steps. -/
def pieceAt (e : Expr) (path : List Nat) : Expr := Id.run do
  let mut e := e
  for i in path do
    let c := e.consumeMData
    if c.isAppOfArity ``And 2 then
      e := if i == 0 then c.appFn!.appArg! else c.appArg!
    else return c
  return e.consumeMData

/-- Whether the proof of `n`, or of a single-caller lemma it applies, rewrites with an `Iff`
lemma whose name mentions the head `pre` (`Metric.tendsto_atTop` for `Filter.Tendsto`): the
statement was put into that form on purpose. -/
def introducedBy (env : Environment) (n pre : Name) (singleCaller : Name → Option Name) : Bool :=
  let key := (pre.getString!).toLower
  let consts (m : Name) : Array Name :=
    ((env.find? m).bind (·.value? (allowOpaque := true))).map (·.getUsedConstants) |>.getD #[]
  let direct := consts n
  let below := direct.filter (singleCaller · == some n)
  let all := direct ++ below.foldl (fun acc m => acc ++ consts m) #[]
  all.any fun c =>
    ((toString c).toLower.splitOn key).length > 1 &&
      (env.find? c).any fun ci => (conclusion ci.type).isAppOf ``Iff

/-! ### Seams -/

def owner (env : Environment) (n : Name) : Name := Id.run do
  let mut m := n
  repeat
    if m.isAnonymous then return n
    if let some ci := env.find? m then
      if userWritten env m ci then return m
    m := m.getPrefix
  return n

/-- Strip `fun`s and annotations. -/
def body (e : Expr) : Expr := Id.run do
  let mut e := e
  repeat
    match e with
    | .lam _ _ b _ | .mdata _ b => e := b
    | _ => break
  return e

/-- Strip annotations at the top. -/
def body1 (e : Expr) : Expr := Id.run do
  let mut e := e
  repeat
    match e with
    | .mdata _ b => e := b
    | _ => break
  return e

/-- Nodes of the proof that serve each piece, when the proof is an anonymous constructor. -/
partial def pieceNodes (e : Expr) (path : List Step) (acc : Array (List Step × Nat)) :
    Array (List Step × Nat) :=
  let e := body e
  if e.isAppOfArity ``And.intro 4 then
    pieceNodes e.appArg! (path ++ [.idx 1])
      (pieceNodes e.appFn!.appArg! (path ++ [.idx 0]) acc)
  else if e.isAppOfArity ``Exists.intro 4 then
    pieceNodes e.appArg! (path ++ [.ex]) (acc.push (path ++ [.wit], size e.appFn!.appArg!))
  else acc.push (path, size e)

def iffDir (e : Expr) : Option (Bool × Name) :=
  let e := e.consumeMData
  let dir := if e.isAppOf ``Iff.mp then some true
    else if e.isAppOf ``Iff.mpr then some false else none
  match dir with
  | some d => if e.getAppNumArgs ≥ 3 then
      match e.getArg! 2 |>.getAppFn with
      | .const c _ => some (d, c)
      | _ => none
    else none
  | none => none

/-- The case splits of a term in prefix order (at most 64): `casesOn`/`rec` of inductive types
with several constructors, matchers with several alternatives, `Or.elim`, `dite`,
`lt_trichotomy` and the like. Splits of `And`, `Exists`, `Eq` and other one-constructor types
do not branch and are left out. -/
partial def skeleton (env : Environment) (e : Expr) : Array String :=
  go e #[]
where
  ctors (ind : Name) : Nat :=
    match env.find? ind with
    | some (.inductInfo i) => i.ctors.length
    | _ => 0
  go (e : Expr) (acc : Array String) : Array String :=
    if acc.size ≥ 64 then acc else
    match e with
    | .app .. =>
      let f := e.getAppFn
      let acc := match f with
        | .const c _ =>
          if let some mi := getMatcherInfoCore? env c then
            if mi.numAlts ≥ 2 then acc.push s!"match/{mi.numAlts}" else acc
          else if isCasesOnRecursor env c || isAuxRecursorWithSuffix env c "recOn" then
            if ctors c.getPrefix ≥ 2 then acc.push s!"{c.getPrefix}.cases" else acc
          else if (env.find? c).any (· matches .recInfo _) then
            if ctors c.getPrefix ≥ 2 then acc.push s!"{c.getPrefix}.rec" else acc
          else if [``Or.elim, `lt_trichotomy, `le_or_lt, `lt_or_le, ``Nat.lt_trichotomy,
              ``dite, ``Decidable.byCases, `em, ``Classical.em, ``Nat.le_total,
              `le_total, ``Nat.eq_zero_or_pos, `lt_or_eq_of_le, `eq_or_lt_of_le].contains c then
            acc.push (toString c)
          else acc
        | _ => acc
      e.getAppArgs.foldl (fun a x => go x a) (go f acc)
    | .lam _ _ b _ | .forallE _ _ b _ | .mdata _ b | .proj _ _ b => go b acc
    | .letE _ _ v b _ => go b (go v acc)
    | _ => acc

structure Unit' where
  owner : Name
  name : Name
  stmt : Expr
  skel : Array String
  nodes : Nat
  deriving Inhabited

/-- `have`-level subproofs of a term with their statements (fingerprints) and skeletons. -/
partial def haves (env : Environment) (minSize : Nat) (e : Expr) (owner : Name) :
    StateM (Array Unit' × Nat) Unit :=
  go e []
where
  go (e : Expr) (types : List Expr) : StateM (Array Unit' × Nat) Unit := do
    let (acc, steps) ← get
    if steps > 200000 then return
    set (acc, steps + 1)
    let record (nm : Name) (t v : Expr) : StateM (Array Unit' × Nat) Unit := do
      if looksProp env t then
        let n := size v
        let sk := skeleton env v
        if n ≥ minSize && sk.size ≥ 2 then
          let u := Unit'.mk owner nm.eraseMacroScopes (fingerprint t types) sk n
          modify fun (a, s) => (a.push u, s)
    match e with
    | .letE nm t v b _ => record nm t v; go v types; go b (t :: types)
    | .app .. =>
      let f := e.getAppFn
      let args := e.getAppArgs
      if let .lam nm t _ _ := f.consumeMData then
        if let some v := args[0]? then record nm t v
      go f types
      for a in args do go a types
    | .lam _ t b _ | .forallE _ t b _ => go b (t :: types)
    | .mdata _ b | .proj _ _ b => go b types
    | _ => pure ()

structure Opts where
  minSize : Nat := 40
  maxDiff : Nat := 2

/-- (d) Hypotheses whose only uses pass them on to other declarations' binders. -/
def threads (env : Environment) (_isProj : Array Bool) (decls : Array (Name × ConstantInfo))
    (out : IO.FS.Stream) : IO Unit := do
  let idx : Std.HashMap Name ConstantInfo := decls.foldl (fun m (n, ci) => m.insert n ci) {}
  -- For each (declaration, binder): real use, and the binders it is passed to.
  let mut real : Std.HashSet (Name × Nat) := {}
  let mut passes : Std.HashMap (Name × Nat) (Array (Name × Nat)) := {}
  let mut props : Array (Name × Nat × Name) := #[]
  for (n, ci) in decls do
    let some v := ci.value? (allowOpaque := true) | continue
    let k := min (lambdaCount v) (binders ci.type).size
    if k == 0 then continue
    let bs := binders ci.type
    let mut e := v
    let mut fvs : Array Expr := #[]
    for i in [0:k] do
      repeat
        match e with
        | .mdata _ b => e := b
        | _ => break
      match e with
      | .lam _ _ b _ =>
        let fv := mkFVar ⟨Name.mkNum `_seams_h i⟩
        fvs := fvs.push fv
        e := b.instantiate1 fv
      | _ => break
    for i in [0:fvs.size] do
      if looksProp env (bs[i]!.2.1) && !bs[i]!.2.2.isInstImplicit then
        props := props.push (n, i, bs[i]!.1)
    let which (x : Expr) : Option Nat := fvs.idxOf? x
    let mut todo := #[e]
    let mut steps := 0
    let mut visited : Std.HashSet Expr := {}
    while !todo.isEmpty && steps < 400000 do
      steps := steps + 1
      let x := todo.back!
      todo := todo.pop
      if !x.hasFVar || visited.contains x then continue
      visited := visited.insert x
      if let some i := which x then
        real := real.insert (n, i)
        continue
      match x with
      | .app .. =>
        let f := x.getAppFn
        let args := x.getAppArgs
        let target := match f with
          | .const c _ => if (idx.get? c).any (fun ci => (ci.value? (allowOpaque := true)).any
              (lambdaCount · > 0)) then some c else none
          | _ => none
        match target with
        | some c =>
          let lc := lambdaCount (((idx.get? c).bind (·.value? (allowOpaque := true))).getD
            (.sort .zero))
          for j in [0:args.size] do
            match which args[j]! with
            | some i =>
              if j < lc then passes := passes.insert (n, i) ((passes.getD (n, i) #[]).push (c, j))
              else real := real.insert (n, i)
            | none => todo := todo.push args[j]!
        | none =>
          todo := todo.push f
          for a in args do todo := todo.push a
      | .lam _ t b _ | .forallE _ t b _ => todo := todo.push t |>.push b
      | .letE _ t v b _ => todo := todo.push t |>.push v |>.push b
      | .mdata _ b | .proj _ _ b => todo := todo.push b
      | _ => pure ()
    -- Unfinished walks: count everything as really used.
    if !todo.isEmpty then
      for i in [0:fvs.size] do real := real.insert (n, i)
  -- Leaves reached from each binder through passes (memoized depth-first search).
  let rec reach (fuel : Nat) (x : Name × Nat) (seen : Std.HashSet (Name × Nat)) :
      Std.HashSet (Name × Nat) × Array Name :=
    match fuel with
    | 0 => (seen, #[])
    | fuel + 1 =>
      if seen.contains x then (seen, #[]) else
      let seen := seen.insert x
      let here := if real.contains x then #[owner env x.1] else #[]
      (passes.getD x #[]).foldl (fun (s, acc) y =>
        let (s, l) := reach fuel y s
        (s, acc ++ l)) (seen, here)
  let range (n : Name) : CoreM (Nat × Nat) := do
    match ← findDeclarationRanges? n with
    | some r => return (r.range.pos.line, r.range.endPos.line)
    | none => return (0, 0)
  -- Chains: for each really used binder, the binders that only carry it there.
  let mut back : Std.HashMap (Name × Nat) (Array (Name × Nat)) := {}
  for (x, ys) in passes.toList do
    for y in ys do back := back.insert y ((back.getD y #[]).push x)
  let isProp : Std.HashSet (Name × Nat) := props.foldl (fun s (n, i, _) => s.insert (n, i)) {}
  let nameOf : Std.HashMap (Name × Nat) Name :=
    props.foldl (fun m (n, i, b) => m.insert (n, i) b) {}
  let mut rows : Array (Nat × String) := #[]
  let mut deadRows : Array (Nat × String) := #[]
  for (leaf, _) in back.toList do
    unless real.contains leaf && isProp.contains leaf do continue
    if (nameOf.getD leaf .anonymous).eraseMacroScopes == `self then continue
    -- Carriers: binders reaching `leaf` through binders that are not really used.
    let mut seen : Std.HashSet (Name × Nat) := {}
    let mut stack := back.getD leaf #[]
    let mut carriers : Array Name := #[]
    while !stack.isEmpty && seen.size < 5000 do
      let x := stack.back!
      stack := stack.pop
      if seen.contains x || real.contains x then continue
      seen := seen.insert x
      let o := owner env x.1
      let theoremOrDef := (idx.get? x.1).any fun ci =>
        userWritten env x.1 ci && (ci matches .thmInfo _ || ci matches .defnInfo _) &&
          !isInstanceCore env x.1
      if o != owner env leaf.1 && theoremOrDef then
        carriers := carriers.push o
      stack := stack ++ back.getD x #[]
    let cs := carriers.toList.eraseDups
    if cs.length < 2 then continue
    let lo := owner env leaf.1
    let (a, b) ← (range lo).toIO' { fileName := "<seams>", fileMap := default } { env }
    let bn := (nameOf.getD leaf .anonymous).eraseMacroScopes
    rows := rows.push (cs.length, s!"thread\t{moduleOf env lo}\t{userName lo}\t{a}\t{b}\t\
      {leaf.2}\t{bn}\tused\t{cs.length}\t{", ".intercalate (cs.map (toString ∘ userName))}")
  -- Dead chains: binders passed on whose chain never reaches a real use.
  let mut live : Std.HashSet (Name × Nat) := {}
  let mut stack : Array (Name × Nat) := real.toArray
  while !stack.isEmpty do
    let x := stack.back!
    stack := stack.pop
    for y in back.getD x #[] do
      unless live.contains y do
        live := live.insert y
        stack := stack.push y
  for (n, i, bn) in props do
    unless (idx.get? n).any (userWritten env n ·) do continue
    if real.contains (n, i) || live.contains (n, i) then continue
    let ps := passes.getD (n, i) #[]
    if ps.isEmpty then continue
    let (a, b) ← (range n).toIO' { fileName := "<seams>", fileMap := default } { env }
    let to := ", ".intercalate (ps.toList.map (fun (c, _) => toString (userName (owner env c)))
      |>.eraseDups)
    deadRows := deadRows.push (1, s!"thread\t{moduleOf env n}\t{userName n}\t{a}\t{b}\t{i}\t\
      {bn.eraseMacroScopes}\tdead\t0\t{to}")
  for (_, r) in rows.qsort (fun x y => x.1 > y.1) do out.putStrLn r
  for (_, r) in deadRows do out.putStrLn r
  let nCarried := rows.size
  let nDead := deadRows.size
  out.putStrLn s!"# hypotheses carried through at least two declarations to one use: {nCarried}; \
    passed on but never used: {nDead}"

/-- (e) Subproofs with the same case-split skeleton and (nearly) the same statement. -/
def skeletons (env : Environment) (o : Opts) (decls : Array (Name × ConstantInfo))
    (out : IO.FS.Stream) : IO Unit := do
  let mut units : Array Unit' := #[]
  for (n, ci) in decls do
    unless ci matches .thmInfo _ do continue
    let some v := ci.value? (allowOpaque := true) | continue
    let own := owner env n
    let sk := skeleton env v
    if userWritten env n ci && sk.size ≥ 2 && size v ≥ o.minSize then
      units := units.push (Unit'.mk own .anonymous ci.type sk (size v))
    let ((), (hs, _)) := (haves env o.minSize v own).run (#[], 0)
    units := units ++ hs
  let mut groups : Std.HashMap String (Array Nat) := {}
  for h : i in [0:units.size] do
    let key := " ".intercalate units[i].skel.toList
    groups := groups.insert key ((groups.getD key #[]).push i)
  let range (n : Name) : CoreM (Nat × Nat) := do
    match ← findDeclarationRanges? n with
    | some r => return (r.range.pos.line, r.range.endPos.line)
    | none => return (0, 0)
  let mut g := 0
  let mut rows : Array (Nat × Array String) := #[]
  for (key, members) in groups.toList do
    if members.size < 2 || members.size > 60 then continue
    -- Pairs whose statements agree up to `maxDiff` leaves; one row per member of a pair.
    for a in [0:members.size] do
      for b in [a+1:members.size] do
        let x := units[members[a]!]!
        let y := units[members[b]!]!
        if x.owner == y.owner && x.name == y.name && x.name.isAnonymous then continue
        match subtreeDiffs (conclusion x.stmt) (conclusion y.stmt) o.maxDiff with
        | some ds =>
          g := g + 1
          let note := if ds.isEmpty then "same statement" else
            ", ".intercalate (ds.toList.map fun (p, q) => s!"{diffStr p} ↔ {diffStr q}")
          let mut lines := #[]
          for (u, nt) in [(x, "-"), (y, note)] do
            let (s, e) ← (range u.owner).toIO' { fileName := "<seams>", fileMap := default } { env }
            let hv := if u.name.isAnonymous then "-" else toString u.name
            lines := lines.push s!"skel\t{g}\t{u.nodes}\t{moduleOf env u.owner}\t\
              {userName u.owner}\t{s}\t{e}\t{hv}\t{key}\t{nt}"
          rows := rows.push (x.nodes + y.nodes, lines)
        | none => pure ()
  for (_, ls) in rows.qsort (fun a b => a.1 > b.1) do
    for l in ls do out.putStrLn l
  out.putStrLn s!"# subproofs with the same skeleton and close statements: {rows.size} pairs"

def run (o : Opts) (imports roots : Array Name) : IO Unit := do
  let env ← importModules (imports.map ({ module := · })) {} (loadExts := true)
  let isProj ← projectModules env
  for r in roots do
    unless env.contains r do throw <| IO.userError s!"unknown root constant {r}"
  let reached := reachable env isProj roots
  let rootSet : NameSet := roots.foldl (·.insert ·) {}
  let mut projectConsts : Array (Name × ConstantInfo) := #[]
  for (n, ci) in env.constants.map₁.toList do
    if inProject env isProj n && reached.contains n then projectConsts := projectConsts.push (n, ci)
  -- Callers, by owner.
  let mut callers : Std.HashMap Name NameSet := {}
  for (n, ci) in projectConsts do
    let own := owner env n
    let mut deps := ci.type.getUsedConstants
    if let some v := ci.value? (allowOpaque := true) then deps := deps ++ v.getUsedConstants
    for d in deps do
      let od := owner env d
      if od != own && inProject env isProj d then
        callers := callers.insert od ((callers.getD od {}).insert own)
  let singleCaller (n : Name) : Option Name :=
    match (callers.getD n {}).toList with
    | [c] => some c
    | _ => none
  let mut structs : Std.HashMap Name Nat := {}
  for (n, ci) in projectConsts do
    if ci matches .inductInfo _ && isStructure env n then
      structs := structs.insert n (getStructureFields env n).size
  -- Tracked: single-caller theorems with at least two pieces.
  let mut tracked : NameSet := {}
  for (n, ci) in projectConsts do
    if ci matches .thmInfo _ && userWritten env n ci && !rootSet.contains n then
      if (singleCaller n).isSome && (leaves env isProj unfoldFuel ci.type).size ≥ 2 then
        tracked := tracked.insert n
  let ctx : Ctx :=
    { env := env, isProj := isProj, structs := structs, allOf := {}, tracked := tracked,
      budget := 4000000, progress := false }
  let progress := (← IO.getEnv "SEAMS_PROGRESS").isSome
  let say (m : String) : IO Unit := if progress then IO.eprintln s!"seams: {m}" else pure ()
  say "walk"
  let (_, st) ← ((walkAll projectConsts rootSet).run ctx).run {}
  say "parts"

  let out ← IO.getStdout
  let range (n : Name) : CoreM (Nat × Nat) := do
    match ← findDeclarationRanges? n with
    | some r => return (r.range.pos.line, r.range.endPos.line)
    | none => return (0, 0)
  let valueOf (n : Name) : Expr := ((env.find? n).bind (·.value? (allowOpaque := true))).getD
    (.sort .zero)
  let act : MetaM Unit := do
    -- (a) Pieces a single caller reads.
    let mut nPart := 0
    for n in tracked.toArray.qsort (·.toString < ·.toString) do
      let some ci := env.find? n | continue
      let some c := singleCaller n | continue
      let marks := st.marks.getD (.thm n) #[]
      let ls := leaves env isProj unfoldFuel ci.type
      let unread := ls.filter (!leafRead marks ·)
      if unread.isEmpty then continue
      let pieces ← leavesPP isProj unfoldFuel ci.type
      let names := (pieces.filter fun (p, _) => !leafRead marks p).map (·.2)
      let pn := pieceNodes (valueOf n) (List.replicate (lambdaCount (valueOf n)) .app) #[]
      let total := pn.foldl (· + ·.2) 0
      let attributed := pn.size > 1
      let un := pn.foldl (fun a (p, k) => if leafRead marks p then a else a + k) 0
      let (a, b) ← range n
      let nodes := if attributed then s!"{un}/{total}" else "-"
      let txt := " ; ".intercalate (names.toList.map (oneLine · 100))
      out.putStrLn s!"part\t{moduleOf env n}\t{userName n}\t{a}\t{b}\t{userName c}\t\
        {ls.size - unread.size}/{ls.size}\t{nodes}\t{txt}"
      nPart := nPart + 1
    if progress then IO.eprintln "seams: undo"
    -- (c) Introduction and elimination through the same `Iff` across a boundary. One walk per
    -- constant collects its `Iff.mp`/`Iff.mpr` applications and its applications of
    -- single-caller theorems.
    let mut nUndo := 0
    let mut found : Std.HashMap Name (Array String) := {}
    for (m, cm) in projectConsts do
      let some v := cm.value? (allowOpaque := true) | continue
      let mut iffs : Array Expr := #[]
      let mut calls : Array Expr := #[]
      let mut todo := #[v]
      let mut seen : Std.HashSet Expr := {}
      while !todo.isEmpty && seen.size < 200000 do
        let x := todo.back!
        todo := todo.pop
        if seen.contains x then continue
        seen := seen.insert x
        if (iffDir x).isSome then iffs := iffs.push x
        if let some h := x.getAppFn.constName? then
          if singleCaller h == some (owner env m) then calls := calls.push x
        match x with
        | .app f a => todo := todo.push f |>.push a
        | .lam _ t b _ | .forallE _ t b _ => todo := todo.push t |>.push b
        | .letE _ t v b _ => todo := todo.push t |>.push v |>.push b
        | .mdata _ b | .proj _ _ b => todo := todo.push b
        | _ => pure ()
      -- Results eliminated at once by a lemma of the conclusion's own namespace, such as
      -- `Filter.Tendsto.eventually` applied to a `Filter.Tendsto` the lemma just proved.
      for x in seen do
        let some e := x.getAppFn.constName? | continue
        let pre := e.getPrefix
        if pre.isAnonymous || basicHeads.contains pre then continue
        for a in x.getAppArgs do
          let (base, path) := stripAnd a
          let some n := base.getAppFn.constName? | continue
          unless packagingHeads.contains pre do continue
          let some nci := env.find? n | continue
          if !(nci matches .thmInfo _) || !inProject env isProj n || owner env n == owner env m then
            continue
          let stmt := pieceAt (conclusion nci.type) path
          if stmt.getAppFn.constName? == some pre && introducedBy env n pre singleCaller then
            found := found.insert n ((found.getD n #[]).push
              s!"{userName (owner env m)}\teliminated\t{e}")
      -- Results: `foo.1 (n …)` where the proof of `n` ends with `foo.2`.
      for x in iffs do
        let some (d, foo) := iffDir x | continue
        let some a := x.getAppArgs[3]? | continue
        let some n := a.getAppFn.constName? | continue
        unless tracked.contains n || singleCaller n == some (owner env m) do continue
        if let some (d', foo') := iffDir (body (valueOf n)) then
          if foo' == foo && d' != d then
            found := found.insert n ((found.getD n #[]).push
              s!"{userName (owner env m)}\tresult\t{foo}")
      -- Arguments: `n … (foo.2 h) …` where `n` uses that parameter only through `foo.1`.
      for call in calls do
        let some n := call.getAppFn.constName? | continue
        let args := call.getAppArgs
        for j in [0:args.size] do
          let some (d, foo) := iffDir args[j]! | continue
          let lv := valueOf n
          if lambdaCount lv ≤ j then continue
          let mut e := lv
          let fv := mkFVar ⟨`_seams_p⟩
          for i in [0:j+1] do
            e := body1 e
            match e with
            | .lam _ _ b _ =>
              let y := if i == j then fv else mkFVar ⟨Name.mkNum `_seams_q i⟩
              e := b.instantiate1 y
            | _ => break
          let mut all := 0
          let mut wrapped := 0
          let mut work := #[e]
          while !work.isEmpty && all ≤ 10000 do
            let x := work.back!
            work := work.pop
            if !x.hasFVar then continue
            if x == fv then all := all + 1; continue
            if let some (d', foo') := iffDir x then
              if foo' == foo && d' != d && x.getAppArgs[3]? == some fv then
                wrapped := wrapped + 1
                all := all + 1
                for y in x.getAppArgs.extract 0 3 do work := work.push y
                continue
            match x with
            | .app f a => work := work.push f |>.push a
            | .lam _ t b _ | .forallE _ t b _ => work := work.push t |>.push b
            | .letE _ t v b _ => work := work.push t |>.push v |>.push b
            | .mdata _ b | .proj _ _ b => work := work.push b
            | _ => pure ()
          if wrapped > 0 && wrapped == all then
            found := found.insert n ((found.getD n #[]).push
              s!"{userName (owner env m)}\targument\t{foo}")
    for (n, ks) in found.toArray.qsort (fun a b => a.1.toString < b.1.toString) do
      let (a, b) ← range n
      for k in ks.toList.eraseDups do
        out.putStrLn s!"undo\t{moduleOf env n}\t{userName n}\t{a}\t{b}\t{k}"
        nUndo := nUndo + 1
    if progress then IO.eprintln "seams: dispatch"
    -- (g) Lemmas that only dispatch: through a few wrappers (`ext`, `fun`), a case split whose
    -- alternatives are each one application of another theorem.
    let mut nDispatch := 0
    for (n, ci) in projectConsts do
      unless ci matches .thmInfo _ && userWritten env n ci do continue
      let some c := singleCaller n | continue
      let mut e := body (valueOf n)
      let mut alts : Array Expr := #[]
      for _ in [0:6] do
        let some h := e.getAppFn.constName? | break
        let args := e.getAppArgs
        if let some mi := getMatcherInfoCore? env h then
          let start := mi.numParams + 1 + mi.numDiscrs
          alts := args.extract start (start + mi.numAlts)
          if alts.size == 1 then e := body alts[0]!; alts := #[]; continue
          break
        if let some (ind, _) := recShape? env h then
          let start := ind.numParams + 2 + ind.numIndices
          if isCasesOnRecursor env h then alts := args.extract start (start + ind.ctors.length)
          if alts.size == 1 then e := body alts[0]!; alts := #[]; continue
          break
        if h == ``Or.elim && args.size ≥ 5 then
          alts := #[args[3]!, args[4]!]
          break
        if h == ``dite && args.size ≥ 5 then
          alts := #[args[3]!, args[4]!]
          break
        -- A wrapper: go into its last argument if that is a `fun`.
        match args.back? with
        | some l@(.lam ..) => e := body l
        | _ => break
      if alts.size < 2 then continue
      let heads := alts.filterMap fun a => (body a).getAppFn.constName?
      if heads.size != alts.size then continue
      let lemmas := heads.filter fun x =>
        x != n && (env.find? x).any (· matches .thmInfo _) && inProject env isProj x &&
          userWritten env x ((env.find? x).get!)
      if lemmas.size != alts.size then continue
      let (a, b) ← range n
      let ls := ", ".intercalate (lemmas.toList.eraseDups.map (toString ∘ userName))
      out.putStrLn s!"dispatch\t{moduleOf env n}\t{userName n}\t{a}\t{b}\t{userName c}\t{ls}"
      nDispatch := nDispatch + 1
    out.putStrLn s!"# single-caller theorems with several pieces: {tracked.size}; \
      read only in part: {nPart}"
    out.putStrLn s!"# introductions undone across a boundary: {nUndo}"
    out.putStrLn s!"# lemmas that only dispatch: {nDispatch}"
  let _ ← act.toIO { fileName := "<seams>", fileMap := default, maxHeartbeats := 0 } { env }
  say "threads"
  -- (d) Hypotheses that are only passed on.
  threads env isProj projectConsts out
  say "skeletons"
  -- (e) Same skeletons.
  skeletons env o projectConsts out

end Seams

partial def parseArgs (o : Seams.Opts) : List String → Option (Seams.Opts × Array Name × Array Name)
  | "--min-size" :: k :: rest => k.toNat?.bind fun k => parseArgs { o with minSize := k } rest
  | "--max-diff" :: k :: rest => k.toNat?.bind fun k => parseArgs { o with maxDiff := k } rest
  | args =>
    match args.span (· != "--") with
    | (mods, "--" :: roots) =>
      if mods.isEmpty || roots.isEmpty then none
      else some (o, mods.toArray.map String.toName, roots.toArray.map String.toName)
    | _ => none

unsafe def main (args : List String) : IO UInt32 := do
  let some (o, imports, roots) := parseArgs {} args
    | IO.eprintln "usage: lake env lean --run Seams.lean [--min-size n] [--max-diff k] \
        <module>... -- <root constant>..."
      return 2
  initSearchPath (← findSysroot)
  enableInitializersExecution
  try
    Seams.run o imports roots
    return 0
  catch e =>
    IO.eprintln s!"seams: {e}"
    return 1
