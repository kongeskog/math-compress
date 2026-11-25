/-
Unused: parts of the declarations that the root constants reach which nothing reads.

Usage, from inside the Lake project directory (the one with `lakefile.toml`):

    lake env lean --run Unused.lean <module>… -- <root>…

The convention is the one of `Reach.lean`: modules to import before `--`, fully qualified root
constants after it. The reached declarations are computed in the same way (types and values,
inductive types with their constructors, only the project's own modules are entered). Only
reached declarations count as readers. Four analyses are reported, one record per line, fields
separated by tabs:

    field    <module> <structure> <field> <first> <last> <flags>
    conjunct <module> <theorem> <k>/<n> <first> <last> <uses> <statement of the conjunct>
    hypconj  <module> <declaration> <binder>#<i> <k>/<n> <first> <last> <statement>
    binder   <module> <declaration> <kind> <first> <last> <i> <binder> <info> <type>
    incomplete <module> <declaration>

`field`: a field of a project structure that no reached declaration reads. A field is read by
its projection function, by a primitive projection, by a `casesOn`/`rec`/`recOn` alternative
whose variable for it is used, or by any other generated declaration of the structure
(`noConfusion`, `injEq`, ...), which counts as reading every field. A field that the type of a
later field mentions is read. Flags: `parent` (a `toB` field of `extends B`), `class`,
`instance` (an instance-implicit field or one declared an instance).

`conjunct`: the statement of a theorem, after its binders, is split at `∧`, `∃` (into the body),
`∀`/`→` (into the conclusion), and through applications of the project's own definitions
(unfolded). Each piece that is not split further is a conjunct; theorems with at least two are
tracked. Every application of the theorem in a reached declaration is followed: `.1`/`.2`,
`And.left`/`And.right`, `And.casesOn`/`And.rec`, `Exists.casesOn`/`Exists.rec`/`Exists.elim`,
matchers, application to further arguments, and `let`/`have`/beta-redexes that name the result
(a name that is never used makes its value dead: what it would read is not counted). Any other
occurrence (an argument to some function, a field of a structure, the result of a proof, a
rewrite with `▸`/`Eq.mpr`) counts as reading every conjunct below it. `uses` is the number of
applications seen. `hypconj` applies the same to the hypotheses of reached theorems and
definitions: a binder whose type splits into several conjuncts and that the value binds with its
own `fun` (not one introduced inside `induction` or a `match`), followed through the value.

`binder`: a binder of a reached theorem or definition whose variable occurs neither in the rest
of the type nor in the value. Instance-implicit binders, instances, structures and generated
declarations are skipped; a binder that the value does not bind with its own `fun` counts as
used.

`incomplete`: a declaration whose term was too large to follow within the step budget. Every
field, theorem and hypothesis it mentions then counts as fully read.

Matchers are handled by unfolding: an application `foo.match_1 motive discrs alts` is evaluated
as the matcher's definition applied to its arguments, with every argument bound lazily, so an
alternative whose variable for a field is unused does not read that field, through any nesting
of patterns. The bodies of matchers, projection functions and other generated declarations are
not read on their own.

Everything reported is a candidate. Conservative means: when the analysis does not understand a
use, it counts as reading everything. What it does not see: uses that leave no trace in terms
(`simp` with `rfl`-lemmas, definitional unfolding by the kernel, instances found but proved by
`rfl`), and whether removing a piece leaves the rest provable. The test is always: change,
build, compare. Not detected: unused conjuncts of `have`s, of `Iff` directions, of hypotheses
introduced inside a proof, and of results that are passed whole to another lemma (a chain of
two lemmas where only the second ignores a piece is reported only for the second).
-/
import Lean

open Lean Meta

namespace Unused

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
  | app
  deriving BEq, Hashable, Inhabited

def Step.toString : Step → String
  | .idx i => s!"{i}"
  | .ex => "E"
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
      | .lam _ _ b _ => (leaves env isProj fuel b).map (.ex :: ·)
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
        withLocalDecl n bi t fun x => do
          return (← leavesPP isProj fuel (b.instantiate1 x)).map fun (p, s) => (.ex :: p, s)
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

def budgetMsg : String := "unused: step budget exceeded"

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
      use t env
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
      use t env
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
          | .path _ => if f == 1 then extend v .ex else pure .none
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
    let r ← bindMinor args[4]! #[.none, hv] ρ
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

/-! ### Main -/

structure Found where
  mod : Name
  name : Name
  first : Nat
  last : Nat

def rangeOf (n : Name) : CoreM (Nat × Nat) := do
  match ← findDeclarationRanges? n with
  | some r => return (r.range.pos.line, r.range.endPos.line)
  | none => return (0, 0)

def binderInfoStr : BinderInfo → String
  | .default => "explicit"
  | .implicit => "implicit"
  | .strictImplicit => "strict-implicit"
  | .instImplicit => "instance"

def run (imports roots : Array Name) : IO Unit := do
  let env ← importModules (imports.map ({ module := · })) {} (loadExts := true)
  let isProj ← projectModules env
  for r in roots do
    unless env.contains r do throw <| IO.userError s!"unknown root constant {r}"
  let progress := (← IO.getEnv "UNUSED_PROGRESS").isSome
  let t0 ← IO.monoMsNow
  let say (msg : String) : IO Unit := do
    if progress then IO.eprintln s!"unused: {msg} ({(← IO.monoMsNow) - t0} ms)"
  say "imported"
  let reached := reachable env isProj roots
  say "reached"
  let rootSet : NameSet := roots.foldl (·.insert ·) {}
  -- Project structures and their generated constants.
  let mut structs : Std.HashMap Name Nat := {}
  let mut projectConsts : Array (Name × ConstantInfo) := #[]
  for (n, ci) in env.constants.map₁.toList do
    if inProject env isProj n then projectConsts := projectConsts.push (n, ci)
  for (n, ci) in projectConsts do
    if ci matches .inductInfo _ && isStructure env n then
      structs := structs.insert n (getStructureFields env n).size
  let mut allOf : Std.HashMap Name Name := {}
  for (n, ci) in projectConsts do
    if ci matches .ctorInfo _ || env.isProjectionFn n || structs.contains n then continue
    if userWritten env n ci then continue
    let mut p := n.getPrefix
    while !p.isAnonymous do
      if structs.contains p then
        allOf := allOf.insert n p
        break
      p := p.getPrefix
  say "structures"
  -- Theorems with at least two conjuncts.
  let mut tracked : NameSet := {}
  for (n, ci) in projectConsts do
    if ci matches .thmInfo _ && reached.contains n && userWritten env n ci
        && !rootSet.contains n then
      if (leaves env isProj unfoldFuel ci.type).size ≥ 2 then tracked := tracked.insert n
  let ctx : Ctx :=
    { env := env, isProj := isProj, structs := structs, allOf := allOf, tracked := tracked,
      budget := 4000000, progress := progress }
  say "tracked theorems"
  let walkList := projectConsts.filter fun (n, _) => reached.contains n
  let (incomplete, st) ← ((walkAll walkList rootSet).run ctx).run {}
  let out ← IO.getStdout
  let act : MetaM Unit := do
    let mut nFields := 0
    let mut nConj := 0
    let mut nHyp := 0
    let mut nBind := 0
    -- Fields.
    let structList := (structs.toArray.filter fun (s, _) =>
      reached.contains s && (env.find? s).any (userWritten env s ·)).qsort
        (fun a b => a.1.toString < b.1.toString)
    for (s, _) in structList do
      let fields := getStructureFields env s
      let some (.inductInfo iv) := env.find? s | continue
      let some (.ctorInfo cv) := env.find? iv.ctors.head! | continue
      -- Field types after the parameters: a field mentioned by a later one is read.
      let mut fts := #[]
      let mut t := cv.type
      for _ in [0:cv.numParams] do
        if let .forallE _ _ b _ := t.consumeMData then t := b
      let fbis := (binders t).map (·.2.2)
      for i in [0:fields.size] do
        let mut later := false
        let mut tail := t
        for _ in [0:i+1] do
          if let .forallE _ _ b _ := tail.consumeMData then tail := b
        -- `tail` holds the later fields and the result; field `i` is variable 0 there.
        let rec countUses (e : Expr) (d : Nat) (left : Nat) : Bool :=
          match left, e.consumeMData with
          | 0, _ => false
          | l + 1, .forallE _ ty b _ => ty.hasLooseBVar d || countUses b (d + 1) l
          | _, _ => false
        later := countUses tail 0 (fields.size - i - 1)
        fts := fts.push later
      for i in [0:fields.size] do
        if st.fields.contains (s, i) || fts[i]! then continue
        let fname := fields[i]!
        let proj := s ++ fname
        let (a, b) ← match ← rangeOf proj with
          | (0, _) => rangeOf s
          | r => pure r
        let mut fl := #[]
        if (isSubobjectField? env s fname).isSome then fl := fl.push "parent"
        if (fbis[i]?.getD .default).isInstImplicit || isInstanceCore env proj then
          fl := fl.push "instance"
        if isClass env s then fl := fl.push "class"
        let fls := if fl.isEmpty then "-" else ",".intercalate fl.toList
        out.putStrLn s!"field\t{moduleOf env s}\t{userName s}\t{fname}\t{a}\t{b}\t{fls}"
        nFields := nFields + 1
    -- Conjuncts of conclusions.
    let thms := (tracked.toArray.qsort (·.toString < ·.toString))
    for n in thms do
      let some ci := env.find? n | continue
      let marks := st.marks.getD (.thm n) #[]
      let ls := leaves env isProj unfoldFuel ci.type
      if ls.all (leafRead marks ·) then continue
      let pieces ← leavesPP isProj unfoldFuel ci.type
      let (a, b) ← rangeOf n
      let uses := st.uses.getD n 0
      for h : k in [0:pieces.size] do
        let (p, txt) := pieces[k]
        if leafRead marks p then continue
        out.putStrLn
          s!"conjunct\t{moduleOf env n}\t{userName n}\t{k+1}/{pieces.size}\t{a}\t{b}\t{uses}\t{txt}"
        nConj := nConj + 1
    -- Conjuncts of hypotheses.
    let declList := (walkList.filter fun (n, ci) =>
      userWritten env n ci && !rootSet.contains n && !isInstanceCore env n).qsort
        (fun a b => a.1.toString < b.1.toString)
    for (n, ci) in declList do
      let bs := binders ci.type
      -- Only hypotheses that the value binds with its own `fun` were followed.
      let bound := lambdaCount ((ci.value? (allowOpaque := true)).getD (.sort .zero))
      for i in [0:min bs.size bound] do
        let (bn, bt, bi) := bs[i]!
        let bn := bn.eraseMacroScopes
        unless isTrackedHyp env isProj bi bt do continue
        let marks := st.marks.getD (.hyp n i) #[]
        let ls := leaves env isProj unfoldFuel bt
        if ls.all (leafRead marks ·) then continue
        let pieces ← forallBoundedTelescope ci.type (some i) fun _ rest => do
          match rest.consumeMData with
          | .forallE _ t _ _ => leavesPP isProj unfoldFuel t
          | _ => return #[]
        let (a, b) ← rangeOf n
        for h : k in [0:pieces.size] do
          let (p, txt) := pieces[k]
          if leafRead marks p then continue
          out.putStrLn s!"hypconj\t{moduleOf env n}\t{userName n}\t{bn}#{i}\t{k+1}/{pieces.size}\
            \t{a}\t{b}\t{txt}"
          nHyp := nHyp + 1
    -- Unused binders.
    for (n, ci) in declList do
      let kind := match ci with
        | .thmInfo _ => "theorem"
        | .defnInfo _ => "def"
        | _ => ""
      if kind.isEmpty || isStructure env n || isClass env n then continue
      let some v := ci.value? (allowOpaque := true) | continue
      let bs := binders ci.type
      let mut tailTy := ci.type
      let mut tailVal := v
      let mut valOk := true
      for i in [0:bs.size] do
        let (bn, _, bi) := bs[i]!
        let tyB := match tailTy.consumeMData with
          | .forallE _ _ b _ => b
          | e => e
        let (valB, ok) := match valOk, tailVal.consumeMData with
          | true, .lam _ _ b _ => (b, true)
          | _, e => (e, false)
        valOk := ok
        let usedTy := tyB.hasLooseBVar 0
        let usedVal := !ok || valB.hasLooseBVar 0
        if !bi.isInstImplicit && !usedTy && !usedVal then
          let (a, b) ← rangeOf n
          let tyStr ← forallBoundedTelescope ci.type (some i) fun _ rest => do
            match rest.consumeMData with
            | .forallE _ t _ _ =>
              try return oneLine (toString (← ppExpr t)) 80 catch _ => return "?"
            | _ => return "?"
          out.putStrLn s!"binder\t{moduleOf env n}\t{userName n}\t{kind}\t{a}\t{b}\t{i}\
            \t{bn.eraseMacroScopes}\
            \t{binderInfoStr bi}\t{tyStr}"
          nBind := nBind + 1
        tailTy := tyB
        tailVal := valB
    for n in incomplete do
      out.putStrLn s!"incomplete\t{moduleOf env n}\t{userName n}"
    out.putStrLn s!"# {reached.size} project constants reached from {roots.size} roots; \
      {walkList.size} walked, {incomplete.size} beyond the step budget"
    out.putStrLn s!"# unused structure fields: {nFields} (in {structList.size} reached structures)"
    out.putStrLn s!"# unused conjuncts of theorem statements: {nConj} \
      (of {thms.size} theorems with several)"
    out.putStrLn s!"# unused conjuncts of hypotheses: {nHyp}"
    out.putStrLn s!"# unused binders: {nBind}"
  let _ ← act.toIO { fileName := "<unused>", fileMap := default, maxHeartbeats := 0 } { env }

end Unused

def parseArgs (args : List String) : Option (Array Name × Array Name) :=
  match args.span (· != "--") with
  | (mods, "--" :: roots) =>
    if mods.isEmpty || roots.isEmpty then none
    else some (mods.toArray.map String.toName, roots.toArray.map String.toName)
  | _ => none

unsafe def main (args : List String) : IO UInt32 := do
  let some (imports, roots) := parseArgs args
    | IO.eprintln "usage: lake env lean --run Unused.lean <module>... -- <root constant>..."
      return 2
  initSearchPath (← findSysroot)
  enableInitializersExecution
  try
    Unused.run imports roots
    return 0
  catch e =>
    IO.eprintln s!"unused: {e}"
    return 1
