//! Project-specific files for the dashboard: prompts, rules and config.

use anyhow::Result;
use hc_core::config::{Config, DenyMode};
use hc_core::model::{metric_defs, GATES};
use serde_json::json;
use std::path::Path;

const CONTRIBUTOR: &str = include_str!("../../../prompts/contributor.md");
const REVIEWER: &str = include_str!("../../../prompts/reviewer.md");
const IDEATOR: &str = include_str!("../../../prompts/ideator.md");
const LEADER: &str = include_str!("../../../prompts/leader.md");

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
    let vars: [(&str, String); 23] = [
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
        ("title_format", cfg.format.title_help()),
        (
            "techniques",
            bullets(cfg.format.techniques.iter().map(|t| format!("`{}`: {}", t.key, t.description))),
        ),
        (
            "protect",
            bullets(cfg.protect.iter().map(|p| if p.description.is_empty() { p.name.clone() } else { p.description.clone() })),
        ),
        (
            "bootstrap_note",
            if cfg.roles.bootstrap {
                "\n\nBootstrap phase: the leaders currently run every agent, including you, from the project's own account, so the pull request may show that account as its author. That is expected. What matters is that you did not write the change and judge it independently; never approve work you produced yourself.".to_string()
            } else {
                String::new()
            },
        ),
        ("max_open_prs", cfg.limits.max_open_prs.to_string()),
        ("reviewer_min_merged", cfg.roles.reviewer_min_merged.to_string()),
        ("ideator_min_merged", cfg.roles.ideator_min_merged.to_string()),
    ];
    let mut out = template.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("{{{{{k}}}}}"), &v);
    }
    out
}

pub fn role_prompt(role: &str, cfg: &Config, site: &str) -> Result<String> {
    let template = match role {
        "contributor" => CONTRIBUTOR,
        "reviewer" => REVIEWER,
        "ideator" => IDEATOR,
        "leader" => LEADER,
        other => anyhow::bail!("unknown role `{other}`"),
    };
    Ok(prompt(template, cfg, site))
}

pub fn site_files(cfg: &Config, out: &Path, site: &str) -> Result<()> {
    // The public site carries one invitation: a one-line prompt that points
    // at the full contributor instructions. The reviewer, ideator and leader
    // prompts stay in the harness repository for those who hold the role.
    std::fs::create_dir_all(out)?;
    let site = if site.ends_with('/') { site.to_string() } else { format!("{site}/") };
    std::fs::write(out.join("join.txt"), prompt(CONTRIBUTOR, cfg, &site))?;
    std::fs::write(
        out.join("prompt.txt"),
        format!(
            "Fetch and follow the instructions at {site}join.txt to join the effort to shorten the machine-checked proof of {} with me\n",
            cfg.project.title
        ),
    )?;

    // Link previews: fill in the page's title, description and absolute URLs.
    let index = out.join("index.html");
    if let Ok(html) = std::fs::read_to_string(&index) {
        let esc = |s: &str| s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;");
        let html = html
            .replace("__OG_TITLE__", &esc(&format!("Shortening the {} proof", cfg.project.title)))
            .replace("__OG_DESC__", &esc(&cfg.project.description))
            .replace("__OG_URL__", &esc(&site));
        std::fs::write(&index, html)?;
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
        "protect": cfg.protect.iter().map(|d| json!({"name": d.name, "description": d.description})).collect::<Vec<_>>(),
        "techniques": cfg.format.techniques,
        "title_format": cfg.format.title_help().replace('`', ""),
        "roles": {
            "bootstrap": cfg.roles.bootstrap,
            "leaders": cfg.roles.leaders,
            "reviewer_min_merged": cfg.roles.reviewer_min_merged,
            "ideator_min_merged": cfg.roles.ideator_min_merged,
            "leader_min_merged": cfg.roles.leader_min_merged,
            "endorsements": cfg.roles.endorsements,
            "leader_endorsements": cfg.roles.leader_endorsements,
        },
        "commands": [
            {"command": "/claim", "who": "anyone", "where": "a queued idea", "does": "Reserve the idea for a few hours."},
            {"command": "/unclaim", "who": "the claimant", "where": "the idea", "does": "Release the claim."},
            {"command": "/retry", "who": "the author or a leader", "where": "a pull request", "does": "Run verification again."},
            {"command": "/approve <commit>", "who": "reviewers and leaders", "where": "a pull request", "does": "Approve exactly that commit."},
            {"command": "/request-changes <reason>", "who": "reviewers and leaders", "where": "a pull request", "does": "Block the change until the same reviewer approves."},
            {"command": "/accept", "who": "ideators and leaders", "where": "a proposed idea", "does": "Put the idea into the queue."},
            {"command": "/decline", "who": "ideators and leaders", "where": "a proposed idea or an application", "does": "Close it; give the reason in the same comment."},
            {"command": "/apply reviewer|ideator|leader", "who": "anyone with enough merged changes", "where": "any issue", "does": "Open an application for leaders to judge."},
            {"command": "/endorse", "who": "leaders", "where": "an application", "does": "Support the application."},
            {"command": "/grant @login reviewer|ideator", "who": "leaders", "where": "any issue", "does": "Give a role directly."},
            {"command": "/revoke @login <role>", "who": "leaders", "where": "any issue", "does": "Take a role away."},
        ],
        "settings": {
            "Primary metric": cfg.score.primary,
            "Minimum improvement": cfg.score.min_delta,
            "Maximum line length": cfg.score.max_line_length,
            "Approvals required": cfg.review.required,
            "Approvals for large changes": cfg.review.required_large,
            "Large change threshold (files)": cfg.review.large_change_files,
            "Open changes per author": cfg.limits.max_open_prs,
            "Minimum account age to apply for a role (days)": cfg.roles.min_account_age_days,
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
