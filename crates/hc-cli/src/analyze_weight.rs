//! `hc analyze weight`: dominator weights of the reached declarations (`tools/lean/Weight.lean`).

use crate::analyze::{default_imports, module_path, print_summary, repo_root};
use crate::analyze_more::{run_lean, tokens_of_lines};
use anyhow::{bail, Result};
use hc_core::config::Config;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const WEIGHT_LEAN: &str = include_str!("../../../tools/lean/Weight.lean");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub module: String,
    pub name: String,
    pub first: usize,
    pub last: usize,
    /// `None` when the node hangs under the virtual root.
    pub idom: Option<String>,
    pub depth: usize,
}

pub fn parse_nodes(out: &str) -> Vec<Node> {
    out.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            match f.as_slice() {
                ["node", m, n, a, b, d, depth, ..] => Some(Node {
                    module: m.to_string(),
                    name: n.to_string(),
                    first: a.parse().ok()?,
                    last: b.parse().ok()?,
                    idom: if *d == "-" { None } else { Some(d.to_string()) },
                    depth: depth.parse().ok()?,
                }),
                _ => None,
            }
        })
        .collect()
}

/// Dominated tokens of each node: its own plus those of everything below it in the dominator
/// tree. Nodes are in reverse postorder (a dominator before what it dominates).
pub fn dominated(nodes: &[Node], own: &[usize]) -> Vec<usize> {
    let idx: BTreeMap<&str, usize> = nodes.iter().enumerate().map(|(i, n)| (n.name.as_str(), i)).collect();
    let mut w = own.to_vec();
    for i in (0..nodes.len()).rev() {
        if let Some(p) = nodes[i].idom.as_deref().and_then(|d| idx.get(d)) {
            w[*p] += w[i];
        }
    }
    w
}

pub struct WeightOptions {
    pub root: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub imports: Vec<String>,
    pub theorems: Vec<String>,
    pub out: Option<PathBuf>,
    pub top: usize,
    pub checks: Vec<String>,
}

pub fn weight(o: WeightOptions) -> Result<()> {
    let root = repo_root(o.root.clone());
    let cfg = Config::load(&o.config.clone().unwrap_or_else(|| root.join("hillclimb.toml")))?;
    let project = root.join(&cfg.project.dir);
    let roots = if o.theorems.is_empty() { cfg.project.theorems.clone() } else { o.theorems.clone() };
    if roots.is_empty() {
        bail!("no root theorems: set `project.theorems` in hillclimb.toml or pass --theorem");
    }
    let imports = default_imports(&roots, &o.imports);
    let mut args = Vec::new();
    for c in &o.checks {
        args.push("--check".to_string());
        args.push(c.clone());
    }
    args.extend(imports.iter().cloned());
    args.push("--".into());
    args.extend(roots.iter().cloned());
    println!("Running Weight.lean in {} (imports: {})", project.display(), imports.join(" "));
    let (stdout, secs) = run_lean("Weight.lean", WEIGHT_LEAN, &project, &args)?;
    let nodes = parse_nodes(&stdout);
    let mut cache = BTreeMap::new();
    let own: Vec<usize> = nodes
        .iter()
        .map(|n| if n.first == 0 { 0 } else { tokens_of_lines(&mut cache, &project, &n.module, n.first, n.last) })
        .collect();
    let w = dominated(&nodes, &own);
    let total: usize = own.iter().sum();
    let top_level: usize = nodes.iter().zip(&w).filter(|(n, _)| n.idom.is_none()).map(|(_, x)| x).sum();
    println!();
    print_summary(&stdout);
    println!(
        "\nReached declarations hold {total} tokens of source; the nodes under the virtual root (the final \
         theorems and what several of them share) dominate {top_level} (must be equal)."
    );
    let share = |x: usize| if total == 0 { 0.0 } else { 100.0 * x as f64 / total as f64 };
    let mut order: Vec<usize> = (0..nodes.len()).collect();
    order.sort_by(|&a, &b| w[b].cmp(&w[a]).then(a.cmp(&b)));
    println!("\nLargest dominated weights (tokens it alone holds up; own tokens; depth):");
    for &i in order.iter().take(o.top) {
        let n = &nodes[i];
        println!(
            "{:>8} {:>5.1}%  own {:>6}  d{:<2}  {}  {}:{}",
            w[i],
            share(w[i]),
            own[i],
            n.depth,
            n.name,
            module_path(&n.module),
            n.first
        );
    }
    println!("\nOutline (dominator tree, three levels, children above 1% of the total):");
    let mut kids: BTreeMap<Option<&str>, Vec<usize>> = BTreeMap::new();
    for (i, n) in nodes.iter().enumerate() {
        kids.entry(n.idom.as_deref()).or_default().push(i);
    }
    fn outline(
        nodes: &[Node],
        w: &[usize],
        kids: &BTreeMap<Option<&str>, Vec<usize>>,
        at: Option<&str>,
        depth: usize,
        total: usize,
    ) {
        let mut ks = kids.get(&at).cloned().unwrap_or_default();
        ks.sort_by(|&a, &b| w[b].cmp(&w[a]));
        for i in ks {
            if w[i] * 100 < total {
                continue;
            }
            println!("{}{} {}  ({:.1}%)", "  ".repeat(depth + 1), w[i], nodes[i].name, 100.0 * w[i] as f64 / total as f64);
            if depth < 2 {
                outline(nodes, w, kids, Some(nodes[i].name.as_str()), depth + 1, total);
            }
        }
    }
    outline(&nodes, &w, &kids, None, 0, total.max(1));
    let checks: Vec<&str> = stdout.lines().filter(|l| l.starts_with("check\t")).collect();
    if !checks.is_empty() {
        println!("\nChecks (dominated, unreachable without it, same set):");
        for c in checks {
            println!("    {}", c.split('\t').skip(1).collect::<Vec<_>>().join("  "));
        }
    }
    println!("\n({secs:.1} s)");
    if let Some(p) = &o.out {
        let mut k = 0;
        let mut s = String::new();
        for line in stdout.lines() {
            s.push_str(line);
            if line.starts_with("node\t") {
                s.push_str(&format!("\t{}\t{}\t{:.2}", own[k], w[k], share(w[k])));
                k += 1;
            }
            s.push('\n');
        }
        std::fs::write(p, s)?;
        println!("Full list written to {} (node records with own tokens, dominated tokens, share added)", p.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_add_up_the_tree() {
        let out = "node\tA\tA.root\t1\t2\t-\t1\n\
                   node\tA\tA.mid\t3\t4\tA.root\t2\n\
                   node\tA\tA.leaf\t5\t6\tA.mid\t3\n\
                   node\tA\tA.shared\t7\t8\t-\t1\n";
        let n = parse_nodes(out);
        assert_eq!(n.len(), 4);
        assert_eq!(n[1].idom.as_deref(), Some("A.root"));
        let w = dominated(&n, &[10, 5, 2, 7]);
        assert_eq!(w, [17, 7, 2, 7]);
        // The top level holds everything.
        assert_eq!(w[0] + w[3], 24);
    }
}
