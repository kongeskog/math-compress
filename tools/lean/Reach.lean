/-
Reach: which of a Lake project's own declarations do the root constants not need?

Usage, from inside the Lake project directory (the one with `lakefile.toml`):

    lake env lean --run Reach.lean <module>… -- <root>…

Everything before `--` is a module to import (for example `Solution`); everything after it is
the fully qualified name of a root constant (for example `Solution.mainTheorem`). The modules
must be built (`lake build`). The program imports them, walks every constant reachable from the
roots through the constants that occur in types and values (an inductive type brings its
constructors, a constructor or recursor its inductive type), and reports the project's own
user-written declarations that the walk never reaches.

The project's own declarations are those whose module has a source file below the current
directory (module `A.B.C` <-> `A/B/C.lean`). The walk does not enter other packages: nothing
in Mathlib can refer to a declaration of the project.

Generated declarations are not reported on their own: internal names (`_proof_1`, `match_1`,
`eq_1`, `_unsafe_rec`, ...), recursors, `casesOn`, `noConfusion`, `below`, `sizeOf` lemmas,
`injEq`, equation lemmas, structure projections and constructors, and anything without a
declaration range. A reached generated declaration still counts: its type and value are walked.

Output, one record per line, fields separated by tabs:

    decl  <module>  <name>  <kind>  <first line>  <last line>  <flags>
    file  <module>  <path>  <unreached>  <user-written declarations>
    whole <module>  <path>
    unimported  <path>

`decl` lists every unreached user-written declaration. `kind` is one of theorem, def, abbrev,
instance, structure, class, inductive, opaque, axiom. `flags` is `-` or a comma-separated list
of reasons why "unreached" may be wrong: `simp` (in some simp set), `instance`, `ext`,
`reducible`, `rfl` (proof is `rfl`), `private`, `derived` (from a `deriving` clause),
`meta` (a parser, macro or other elaboration-time object), and `attr:<name>` for
each attribute written in the declaration's own modifiers. `file` gives totals per module that
has at least one user-written declaration; `whole` marks modules in which every one is
unreached; `unimported` lists `.lean` files below the current directory that no imported module
loads. Lines starting with `# ` are a human-readable summary.

Caveats. This is a list of candidates; the test is always: delete, build, compare.
* A lemma proved by `rfl` and used only by `simp` or `dsimp` leaves no trace in proof terms and
  looks unreached even though removing it can break a proof. The same holds for anything that
  affects elaboration without appearing in a term: instances that are tried but not chosen,
  simp lemmas, `@[ext]` lemmas, unification hints, macros and notation.
  Flag `rfl` marks theorems whose proof is `rfl`, `Eq.refl` or `Iff.rfl` under binders.
* Declarations used only by `#eval`, `example` or tests also look unreached.
-/
import Lean

open Lean Meta

namespace Reach

/-- The source file of a module, relative to the current directory. -/
def modulePath (mod : Name) : System.FilePath :=
  let parts := mod.components.map fun
    | .str _ s => s
    | n => n.toString
  (System.mkFilePath parts).addExtension "lean"

/-- `isProj[i]` is true when module `i` of the environment has a source file here. -/
def projectModules (env : Environment) : IO (Array Bool) :=
  env.header.moduleNames.mapM fun mod => (modulePath mod).pathExists

def inProject (env : Environment) (isProj : Array Bool) (n : Name) : Bool :=
  match env.getModuleIdxFor? n with
  | some i => isProj.getD i.toNat false
  | none => false

/-- Constants a constant depends on directly. -/
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

/-- Every project constant reachable from `roots`. Non-project constants are not entered. -/
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

/-- `s` is `pre` followed only by digits and underscores. -/
def numbered (s pre : String) : Bool :=
  s.startsWith pre && (s.drop pre.length).all fun c => c.isDigit || c == '_'

def internalComponent (s : String) : Bool :=
  s.startsWith "_" || numbered s "eq_" || numbered s "match_" || numbered s "proof_"
    || numbered s "omega_" || numbered s "eq_def"

/-- Whether a project constant is something a person wrote, as opposed to generated. -/
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
  -- Auxiliary definitions of an instance, such as `instDecidableEqT.decEq` from `deriving`.
  if isInstanceCore env n.getPrefix then return false
  return true

/-- A proof that is `rfl` under its binders: such a lemma is invisible when `simp` uses it. -/
def isRflProof (ci : ConstantInfo) : Bool :=
  match ci with
  | .thmInfo t =>
    let rec strip : Expr → Expr
      | .lam _ _ b _ => strip b
      | .mdata _ b => strip b
      | e => e
    let head := (strip t.value).getAppFn
    [``Eq.refl, ``rfl, ``Iff.refl, ``Iff.rfl, ``HEq.refl, ``HEq.rfl].any (head.isConstOf ·)
  | _ => false

def kindOf (env : Environment) (n : Name) (ci : ConstantInfo) : String :=
  match ci with
  | .thmInfo _ => "theorem"
  | .axiomInfo _ => "axiom"
  | .opaqueInfo _ => "opaque"
  | .inductInfo _ =>
    if isClass env n then "class" else if isStructure env n then "structure" else "inductive"
  | .defnInfo _ =>
    if isInstanceCore env n then "instance"
    else if getReducibilityStatusCore env n matches .reducible then "abbrev"
    else "def"
  | _ => "other"

/-- Attribute names written in the modifiers `@[...]` of a declaration's source text. -/
def writtenAttrs (header : String) : Array String := Id.run do
  let mut out := #[]
  for chunk in (header.splitOn "@[").drop 1 do
    let inside := (chunk.splitOn "]").headD ""
    for a in inside.splitOn "," do
      let words := (a.trimAscii.toString.splitOn " ").filter (· ≠ "")
      let words := words.filter fun w => w ∉ ["scoped", "local", "-", "←", "<-", "high", "low"]
      if let some w := words.head? then out := out.push w
  return out

def metaTypes : List Name :=
  [``ParserDescr, ``TrailingParserDescr, ``Macro, ``PrettyPrinter.Unexpander,
   ``Lean.PrettyPrinter.Delaborator.Delab, ``Lean.Elab.Term.TermElab,
   ``Lean.Elab.Command.CommandElab, ``Lean.Elab.Tactic.Tactic]

def isMetaObject (ci : ConstantInfo) : Bool :=
  match ci.type.getAppFn.constName? with
  | some c => metaTypes.contains c || (`Lean).isPrefixOf c && ci.type.isConst
  | none => false

def simpSets : CoreM (Array SimpTheorems) := do
  let mut out := #[]
  for (_, ext) in (← simpExtensionMapRef.get).toList do
    out := out.push (← ext.getTheorems)
  return out

def flagsOf (n : Name) (ci : ConstantInfo) (sets : Array SimpTheorems) (header : String) :
    CoreM (Array String) := do
  let env ← getEnv
  let mut fl := #[]
  if sets.any (·.isLemma (.decl n)) || sets.any (·.isDeclToUnfold n) then fl := fl.push "simp"
  if isInstanceCore env n then fl := fl.push "instance"
  if (Lean.Meta.Ext.extExtension.getState env).contains n then fl := fl.push "ext"
  if getReducibilityStatusCore env n matches .reducible then fl := fl.push "reducible"
  if isMetaObject ci then fl := fl.push "meta"
  if isRflProof ci then fl := fl.push "rfl"
  if isPrivateName n then fl := fl.push "private"
  if (header.splitOn "\n").any (·.trimAsciiStart.startsWith "deriving ") then
    fl := fl.push "derived"
  for a in writtenAttrs header do
    let f := s!"attr:{a}"
    if !fl.contains f then fl := fl.push f
  return fl

/-- Lines of a source file, read once. -/
def fileLines (cache : IO.Ref (Std.HashMap String (Array String))) (p : System.FilePath) :
    IO (Array String) := do
  if let some ls := (← cache.get)[p.toString]? then return ls
  let ls := (← IO.FS.readFile p).splitOn "\n" |>.toArray
  cache.modify (·.insert p.toString ls)
  return ls

/-- Text from the start of a declaration up to its name: doc comment and modifiers. -/
def headerText (lines : Array String) (r : DeclarationRanges) : String := Id.run do
  let a := r.range.pos.line
  let b := r.selectionRange.pos.line
  let mut s := ""
  for i in [a:b+1] do
    s := s ++ lines.getD (i - 1) "" ++ "\n"
  return s

partial def leanFiles (dir : System.FilePath) : IO (Array System.FilePath) := do
  let mut out := #[]
  for e in ← dir.readDir do
    if e.fileName.startsWith "." then continue
    if ← e.path.isDir then
      out := out ++ (← leanFiles e.path)
    else if e.path.extension == some "lean" && e.fileName != "lakefile.lean" then
      out := out.push e.path
  return out

structure Decl where
  mod : Name
  name : Name
  kind : String
  first : Nat
  last : Nat
  flags : Array String
  reached : Bool

def run (imports roots : Array Name) : IO Unit := do
  let env ← importModules (imports.map ({ module := · })) {} (loadExts := true)
  let isProj ← projectModules env
  for r in roots do
    unless env.contains r do throw <| IO.userError s!"unknown root constant {r}"
  let reached := reachable env isProj roots
  let cache ← IO.mkRef ({} : Std.HashMap String (Array String))
  let act : CoreM (Array Decl) := do
    let sets ← simpSets
    let mut decls := #[]
    for (n, ci) in env.constants.map₁.toList do
      unless inProject env isProj n && userWritten env n ci do continue
      let some r ← findDeclarationRanges? n | continue
      let some mod := env.getModuleIdxFor? n >>= (env.header.moduleNames[·.toNat]?) | continue
      let lines ← fileLines cache (modulePath mod)
      let fl ← flagsOf n ci sets (headerText lines r)
      decls := decls.push
        { mod, name := n, kind := kindOf env n ci, first := r.range.pos.line,
          last := r.range.endPos.line, flags := fl, reached := reached.contains n }
    return decls
  let (decls, _) ← act.toIO { fileName := "<reach>", fileMap := default } { env }
  let decls := decls.qsort fun a b =>
    a.mod.toString < b.mod.toString || (a.mod == b.mod && a.first < b.first)
  let out ← IO.getStdout
  let mut perMod : Std.HashMap Name (Nat × Nat) := {}
  let mut modOrder : Array Name := #[]
  for d in decls do
    let (u, t) := perMod.getD d.mod (0, 0)
    if t == 0 then modOrder := modOrder.push d.mod
    perMod := perMod.insert d.mod (u + (if d.reached then 0 else 1), t + 1)
    unless d.reached do
      let fl := if d.flags.isEmpty then "-" else ",".intercalate d.flags.toList
      let name := (privateToUserName? d.name).getD d.name
      out.putStrLn s!"decl\t{d.mod}\t{name}\t{d.kind}\t{d.first}\t{d.last}\t{fl}"
  let mut whole := #[]
  let mut partly := #[]
  for m in modOrder do
    let (u, t) := perMod.getD m (0, 0)
    out.putStrLn s!"file\t{m}\t{modulePath m}\t{u}\t{t}"
    if u == t then whole := whole.push m
    else if u > 0 then partly := partly.push (m, u, t)
  for m in whole do
    out.putStrLn s!"whole\t{m}\t{modulePath m}"
  let loaded : Std.HashSet String :=
    env.header.moduleNames.foldl (fun s m => s.insert (modulePath m).toString) {}
  let mut unimported0 := #[]
  for p in ← leanFiles "." do
    let rel := (p.toString.dropPrefix "./").toString
    unless loaded.contains rel do unimported0 := unimported0.push rel
  let unimported := unimported0.qsort (· < ·)
  for p in unimported do
    out.putStrLn s!"unimported\t{p}"
  let total := decls.size
  let unreached := decls.filter (!·.reached)
  let flagged := unreached.filter (!·.flags.isEmpty)
  out.putStrLn s!"# {reached.size} project constants reached from {roots.size} roots"
  out.putStrLn s!"# {unreached.size} of {total} user-written declarations unreached \
    ({flagged.size} flagged), in {perMod.size} modules"
  out.putStrLn s!"# whole modules unreached: {whole.size}"
  for m in whole do
    let (_, t) := perMod.getD m (0, 0)
    out.putStrLn s!"#   {modulePath m}  ({t} declarations)"
  out.putStrLn s!"# modules with some unreached declarations: {partly.size}"
  for (m, u, t) in partly do
    out.putStrLn s!"#   {modulePath m}  {u} of {t}"
  if !unimported.isEmpty then
    out.putStrLn s!"# files not loaded by the imports: {unimported.size}"
    for p in unimported do
      out.putStrLn s!"#   {p}"

end Reach

/-- Splits `<module>… -- <root>…`. -/
def parseArgs (args : List String) : Option (Array Name × Array Name) :=
  match args.span (· != "--") with
  | (mods, "--" :: roots) =>
    if mods.isEmpty || roots.isEmpty then none
    else some (mods.toArray.map String.toName, roots.toArray.map String.toName)
  | _ => none

unsafe def main (args : List String) : IO UInt32 := do
  let some (imports, roots) := parseArgs args
    | IO.eprintln "usage: lake env lean --run Reach.lean <module>... -- <root constant>..."
      return 2
  initSearchPath (← findSysroot)
  enableInitializersExecution
  try
    Reach.run imports roots
    return 0
  catch e =>
    IO.eprintln s!"reach: {e}"
    return 1
