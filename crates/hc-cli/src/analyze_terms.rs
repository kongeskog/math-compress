//! `hc analyze terms`: kernel term sizes of the project's declarations (`tools/lean/Terms.lean`).

use crate::analyze::{default_imports, module_path, print_summary, repo_root};
use crate::analyze_more::{run_lean, tokens_of_lines};
use anyhow::{bail, Context, Result};
use hc_core::config::Config;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const TERMS_LEAN: &str = include_str!("../../../tools/lean/Terms.lean");

/// One declaration: module, name, type dag, value dag, value tree, source tokens (if known).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub module: String,
    pub name: String,
    pub first: usize,
    pub last: usize,
    pub type_dag: u128,
    pub value_dag: u128,
    pub value_tree: u128,
    pub tokens: Option<usize>,
}

/// Parse `term` records; a tenth field, written by `--out`, is the source tokens.
pub fn parse_rows(out: &str) -> Vec<Row> {
    out.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            if f.first() != Some(&"term") || f.len() < 9 {
                return None;
            }
            Some(Row {
                module: f[1].to_string(),
                name: f[2].to_string(),
                first: f[3].parse().ok()?,
                last: f[4].parse().ok()?,
                type_dag: f[5].parse().ok()?,
                value_dag: f[6].parse().ok()?,
                value_tree: f[7].parse().ok()?,
                tokens: f.get(9).and_then(|t| t.parse().ok()),
            })
        })
        .collect()
}

/// Per-module sums: (term dag = type + value, tree, tokens).
pub fn by_module(rows: &[Row]) -> BTreeMap<String, (u128, u128, usize)> {
    let mut m: BTreeMap<String, (u128, u128, usize)> = BTreeMap::new();
    for r in rows {
        let e = m.entry(r.module.clone()).or_default();
        e.0 += r.type_dag + r.value_dag;
        e.1 += r.value_tree;
        e.2 += r.tokens.unwrap_or(0);
    }
    m
}

/// Differences between two runs, keyed by name: (before, after) of (dag, tokens).
pub fn changes(
    old: &BTreeMap<String, (u128, usize)>,
    new: &BTreeMap<String, (u128, usize)>,
) -> Vec<(String, i128, i64)> {
    let mut keys: Vec<&String> = old.keys().chain(new.keys()).collect();
    keys.sort();
    keys.dedup();
    let mut v: Vec<(String, i128, i64)> = keys
        .into_iter()
        .map(|k| {
            let (a, ta) = old.get(k).copied().unwrap_or((0, 0));
            let (b, tb) = new.get(k).copied().unwrap_or((0, 0));
            (k.clone(), b as i128 - a as i128, tb as i64 - ta as i64)
        })
        .filter(|(_, d, t)| *d != 0 || *t != 0)
        .collect();
    v.sort_by(|a, b| b.1.abs().cmp(&a.1.abs()).then(a.0.cmp(&b.0)));
    v
}

pub struct TermsOptions {
    pub root: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub imports: Vec<String>,
    pub theorems: Vec<String>,
    pub out: Option<PathBuf>,
    pub top: usize,
    pub compare: Option<PathBuf>,
}

/// A one-line reading of a change in tokens and term size.
pub fn verdict(term: i128, tokens: i64) -> &'static str {
    match (tokens.signum(), term.signum()) {
        (-1, 1) => "fewer tokens, larger terms",
        (-1, -1) => "fewer tokens, smaller terms",
        (-1, 0) => "fewer tokens, same terms",
        (1, 1) => "more tokens, larger terms",
        (1, -1) => "more tokens, smaller terms",
        (1, 0) => "more tokens, same terms",
        (0, 1) => "same tokens, larger terms",
        (0, -1) => "same tokens, smaller terms",
        _ => "unchanged",
    }
}

fn print_changes(title: &str, v: &[(String, i128, i64)], top: usize, tokens: bool) {
    println!("\n{title} ({} changed): term nodes, tokens", v.len());
    for (k, d, t) in v.iter().take(top) {
        if tokens {
            println!("{d:>+12} {t:>+8}  {k}  ({})", verdict(*d, *t));
        } else {
            println!("{d:>+12} {:>8}  {k}", "?");
        }
    }
}

pub fn terms(o: TermsOptions) -> Result<()> {
    let root = repo_root(o.root.clone());
    let cfg = Config::load(&o.config.clone().unwrap_or_else(|| root.join("hillclimb.toml")))?;
    let project = root.join(&cfg.project.dir);
    let roots = if o.theorems.is_empty() { cfg.project.theorems.clone() } else { o.theorems.clone() };
    if roots.is_empty() {
        bail!("no root theorems: set `project.theorems` in hillclimb.toml or pass --theorem");
    }
    let imports = default_imports(&roots, &o.imports);
    let mut args: Vec<String> = imports.clone();
    args.push("--".into());
    args.extend(roots.iter().cloned());
    println!("Running Terms.lean in {} (imports: {})", project.display(), imports.join(" "));
    let (stdout, secs) = run_lean("Terms.lean", TERMS_LEAN, &project, &args)?;
    let mut rows = parse_rows(&stdout);
    let mut cache = BTreeMap::new();
    for r in &mut rows {
        r.tokens =
            Some(if r.first == 0 { 0 } else { tokens_of_lines(&mut cache, &project, &r.module, r.first, r.last) });
    }
    println!();
    print_summary(&stdout);
    report(&rows, o.top);
    if let Some(prev) = &o.compare {
        let old = parse_rows(&std::fs::read_to_string(prev).with_context(|| format!("reading {}", prev.display()))?);
        compare(&old, &rows, o.top);
    }
    println!("\n({secs:.1} s)");
    if let Some(p) = &o.out {
        write_out(p, &stdout, &rows)?;
        println!("Full list written to {} (term records with tokens added)", p.display());
    }
    Ok(())
}

fn report(rows: &[Row], top: usize) {
    let dag: u128 = rows.iter().map(|r| r.type_dag + r.value_dag).sum();
    let tree: u128 = rows.iter().map(|r| r.value_tree).sum();
    let tokens: usize = rows.iter().filter_map(|r| r.tokens).sum();
    println!("\nProject: {dag} term nodes (types and values, sharing counted once per constant), {tree} as trees, {tokens} source tokens of these declarations.");
    let mut mods: Vec<(String, (u128, u128, usize))> = by_module(rows).into_iter().collect();
    mods.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    println!("\nLargest modules: term nodes, tokens, nodes per token");
    for (m, (d, _, t)) in mods.iter().take(top) {
        let ratio = if *t == 0 { 0.0 } else { *d as f64 / *t as f64 };
        println!("{d:>12} {t:>8} {ratio:>7.1}  {}", module_path(m));
    }
    let mut rs: Vec<&Row> = rows.iter().collect();
    rs.sort_by(|a, b| (b.type_dag + b.value_dag).cmp(&(a.type_dag + a.value_dag)));
    println!("\nLargest declarations: term nodes, tokens");
    for r in rs.iter().take(top) {
        println!(
            "{:>12} {:>8}  {}  {}:{}",
            r.type_dag + r.value_dag,
            r.tokens.unwrap_or(0),
            r.name,
            module_path(&r.module),
            r.first
        );
    }
}

fn compare(old: &[Row], new: &[Row], top: usize) {
    let decls = |rs: &[Row]| -> BTreeMap<String, (u128, usize)> {
        rs.iter().map(|r| (r.name.clone(), (r.type_dag + r.value_dag, r.tokens.unwrap_or(0)))).collect()
    };
    let mods = |rs: &[Row]| -> BTreeMap<String, (u128, usize)> {
        by_module(rs).into_iter().map(|(m, (d, _, t))| (m, (d, t))).collect()
    };
    let (od, nd) = (decls(old), decls(new));
    let dt: i128 = nd.values().map(|x| x.0 as i128).sum::<i128>() - od.values().map(|x| x.0 as i128).sum::<i128>();
    let tt: i64 = nd.values().map(|x| x.1 as i64).sum::<i64>() - od.values().map(|x| x.1 as i64).sum::<i64>();
    println!("\nCompared with the earlier run: {dt:+} term nodes, {tt:+} tokens: {}.", verdict(dt, tt));
    let tokens = old.iter().all(|r| r.tokens.is_some()) && new.iter().all(|r| r.tokens.is_some());
    let mut per_file = changes(&mods(old), &mods(new));
    per_file.sort_by(|a, b| a.0.cmp(&b.0));
    print_changes("Files", &per_file, usize::MAX, tokens);
    print_changes("Declarations", &changes(&od, &nd), top, tokens);
}

fn write_out(p: &Path, stdout: &str, rows: &[Row]) -> Result<()> {
    let mut k = 0;
    let mut s = String::new();
    for line in stdout.lines() {
        s.push_str(line);
        if line.starts_with("term\t") {
            s.push_str(&format!("\t{}", rows[k].tokens.unwrap_or(0)));
            k += 1;
        }
        s.push('\n');
    }
    std::fs::write(p, s)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sums_and_compares() {
        let a = "term\tA\tA.f\t1\t3\t10\t100\t400\t2\t30\nterm\tB\tB.g\t5\t6\t4\t20\t20\t1\t9\n# x\n";
        let b = "term\tA\tA.f\t1\t2\t10\t160\t900\t2\t12\nterm\tC\tC.h\t1\t1\t1\t1\t1\t1\t1\n";
        let (ra, rb) = (parse_rows(a), parse_rows(b));
        assert_eq!(ra.len(), 2);
        assert_eq!(ra[0].tokens, Some(30));
        assert_eq!(by_module(&ra)["A"], (110, 400, 30));
        let m = |rs: &[Row]| -> BTreeMap<String, (u128, usize)> {
            rs.iter().map(|r| (r.name.clone(), (r.type_dag + r.value_dag, r.tokens.unwrap_or(0)))).collect()
        };
        let c = changes(&m(&ra), &m(&rb));
        assert_eq!(c[0], ("A.f".to_string(), 60, -18));
        assert_eq!(c[1], ("B.g".to_string(), -24, -9));
        assert_eq!(c[2], ("C.h".to_string(), 2, 1));
        assert_eq!(verdict(60, -18), "fewer tokens, larger terms");
        assert_eq!(verdict(-5, -1), "fewer tokens, smaller terms");
        assert_eq!(verdict(0, 0), "unchanged");
    }
}
