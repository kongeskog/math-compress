//! Project-specific files for the dashboard: prompts, rules and config.

use anyhow::Result;
use hc_core::config::{Config, DenyMode};
use hc_core::model::{metric_defs, GATES};
use serde_json::json;
use std::path::Path;

const CONTRIBUTOR: &str = include_str!("../../../prompts/contributor.md");
const REVIEWER: &str = include_str!("../../../prompts/reviewer.md");
const IDEATOR: &str = include_str!("../../../prompts/ideator.md");

fn bullets(items: impl Iterator<Item = String>) -> String {
    items.map(|i| format!("- {i}")).collect::<Vec<_>>().join("\n")
}

pub fn prompt(template: &str, cfg: &Config, site: &str) -> String {
    let p = &cfg.project;
    let deny = bullets(
        cfg.deny
            .iter()
            .map(|d| if d.description.is_empty() { d.name.clone() } else { d.description.clone() })
            .chain(std::iter::once(format!(
                "More lines longer than {} characters than there are today.",
                cfg.score.max_line_length
            ))),
    );
    let guards = cfg
        .score
        .guards
        .iter()
        .map(|g| format!("`{}` (at most +{}%)", g.metric, g.max_increase_pct))
        .collect::<Vec<_>>()
        .join(", ");
    let build = p.local_build.iter().map(|c| format!("   `{c}`")).collect::<Vec<_>>().join("\n");
    let theorems = p.theorems.iter().map(|t| format!("`{t}`")).collect::<Vec<_>>().join(", ");
    let vars: [(&str, String); 16] = [
        ("title", p.title.clone()),
        ("repo", p.repo.clone()),
        ("dir", p.dir.clone()),
        ("branch", p.branch.clone()),
        ("harness_repo", p.harness_repo.clone()),
        ("site", site.to_string()),
        ("primary", cfg.score.primary.clone()),
        ("min_delta", cfg.score.min_delta.to_string()),
        ("guards", guards),
        ("frozen", bullets(p.frozen.iter().map(|f| format!("`{f}`")))),
        ("deny", deny),
        ("build", build),
        ("theorems", theorems),
        ("lease_hours", cfg.ideas.lease_hours.to_string()),
        ("review_required", cfg.review.required.to_string()),
        ("review_large", cfg.review.required_large.to_string()),
    ];
    let mut out = template.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("{{{{{k}}}}}"), &v);
    }
    out
}

pub fn site_files(cfg: &Config, out: &Path, site: &str) -> Result<()> {
    let prompts = out.join("prompts");
    std::fs::create_dir_all(&prompts)?;
    for (name, template) in [("contributor", CONTRIBUTOR), ("reviewer", REVIEWER), ("ideator", IDEATOR)] {
        std::fs::write(prompts.join(format!("{name}.md")), prompt(template, cfg, site))?;
    }

    let rules = json!({
        "gates": GATES.iter().map(|g| json!({"key": g.0, "label": g.1, "description": g.2})).collect::<Vec<_>>(),
        "metrics": metric_defs(),
        "frozen": cfg.project.frozen,
        "theorems": cfg.project.theorems,
        "deny": cfg.deny.iter().map(|d| json!({
            "name": d.name,
            "mode": match d.mode { DenyMode::Forbid => "forbid", DenyMode::NoIncrease => "no_increase" },
            "description": d.description.replace('`', ""),
        })).collect::<Vec<_>>(),
        "settings": {
            "Primary metric": cfg.score.primary,
            "Minimum improvement": cfg.score.min_delta,
            "Maximum line length": cfg.score.max_line_length,
            "Approvals required": cfg.review.required,
            "Approvals for large changes": cfg.review.required_large,
            "Large change threshold (files)": cfg.review.large_change_files,
            "Reviewer minimum account age (days)": cfg.review.min_account_age_days,
            "Idea claim length (hours)": cfg.ideas.lease_hours,
        },
    });
    std::fs::write(out.join("rules.json"), serde_json::to_string_pretty(&rules)?)?;

    let config = json!({
        "repo": cfg.project.repo,
        "harnessRepo": cfg.project.harness_repo,
        "dataUrl": format!("https://raw.githubusercontent.com/{}/hillclimb-data", cfg.project.repo),
    });
    std::fs::write(out.join("config.json"), serde_json::to_string_pretty(&config)?)?;
    Ok(())
}
