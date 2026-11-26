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
const COORDINATOR: &str = include_str!("../../../prompts/coordinator.md");

fn bullets(items: impl Iterator<Item = String>) -> String {
    items.map(|i| format!("- {i}")).collect::<Vec<_>>().join("\n")
}

/// Keep or drop the passages a template marks with a flag: `{{#flag}}…{{/flag}}`
/// stays when the flag is set, `{{^flag}}…{{/flag}}` when it is not. A marker
/// that stands alone on its line takes the line with it. Passages of one
/// flag do not nest.
fn conditional(template: &str, flag: &str, set: bool) -> String {
    let close = format!("{{{{/{flag}}}}}");
    let mut out = String::new();
    let mut rest = template;
    // After a marker at the start of a line, the line break belongs to it.
    let own_line = |before: &str, after: &str| (before.is_empty() || before.ends_with('\n')) && after.starts_with('\n');
    loop {
        let next = [(format!("{{{{#{flag}}}}}"), set), (format!("{{{{^{flag}}}}}"), !set)]
            .into_iter()
            .filter_map(|(open, keep)| rest.find(&open).map(|at| (at, open.len(), keep)))
            .min();
        let Some((at, len, keep)) = next else { break };
        out.push_str(&rest[..at]);
        let mut inner = &rest[at + len..];
        if own_line(&out, inner) {
            inner = &inner[1..];
        }
        let end = inner.find(&close).unwrap_or(inner.len());
        let body = &inner[..end];
        if keep {
            out.push_str(body);
        }
        rest = inner.get(end + close.len()..).unwrap_or("");
        if own_line(body, rest) && (body.is_empty() || body.ends_with('\n')) && (keep || out.is_empty() || out.ends_with('\n')) {
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    out
}

/// The gates of an arena: the table without the review row where no change
/// needs an approval.
pub fn gate_table(cfg: &Config) -> Vec<(&'static str, &'static str, String)> {
    let r = &cfg.review;
    GATES
        .iter()
        .filter(|g| g.0 != "review" || r.on())
        .map(|g| {
            let description = match g.0 {
                "review" if r.required == 0 => format!(
                    "A change of more than {} files is approved by {} vetted reviewer(s) other than the author; smaller changes need no approval. A request for changes by a leader or a vetted reviewer blocks any change until the same person approves it.",
                    r.large_change_files, r.required_large
                ),
                _ => g.2.to_string(),
            };
            (g.0, g.1, description)
        })
        .collect()
}

/// What `hc prompt reviewer` says in an arena where no change needs an approval.
fn no_review(cfg: &Config) -> String {
    let p = &cfg.project;
    format!(
        "This arena does not use review: {} (https://github.com/{}).\n\n\
         Its `hillclimb.toml` sets `[review] required = 0`, so no change needs an approval. A change merges when every automatic gate has passed for its head commit on the current `{}`; there is nothing for a reviewer to approve, and an `/approve` comment merges nothing.\n\n\
         What remains is a brake. A leader (listed in `hillclimb.toml`; a vetted reviewer too, if the arena has any from an earlier phase) who sees a change that should not merge comments the reason and `/request-changes` on the pull request. The change then does not merge, whatever its gates say and whatever is pushed to it, until the same person comments `/approve <commit>` with the full hash of the head commit as it then stands. The instructions for that are in `hc prompt leader`.\n\n\
         To help with the proof, contribute changes (`hc prompt contributor`) or ideas (`hc prompt ideator`). If you read a change and find something wrong with it, say so on the pull request; a leader can act on it.\n",
        p.title, p.repo, p.branch
    )
}

pub fn prompt(template: &str, cfg: &Config, site: &str) -> String {
    let p = &cfg.project;
    let deny = bullets(
        cfg.deny
            .iter()
            .map(|d| if d.description.is_empty() { d.name.clone() } else { d.description.clone() })
            .chain([
                format!("More lines longer than {} characters than there are today.", cfg.score.max_line_length),
                format!(
                    "Any line you add or edit that is longer than {} characters, ends in whitespace, or holds a control character or one that does not show (an edited line counts as added, also where the old line was too long).",
                    cfg.score.max_line_length
                ),
                "A comment or doc comment that still names a declaration you deleted.".to_string(),
                match cfg.limits.max_added_comment_chars {
                    0 => "A link (`https://…`, any `scheme://`, or `www.`) on a line you add, in a comment, a string or a name.".to_string(),
                    max => format!(
                        "More than {max} characters of comments, string literals and names in `«…»` on the lines one change adds (an edited line counts in full; a moved one does not), or a link (`https://…`, any `scheme://`, or `www.`) on a line you add."
                    ),
                },
                "A string, a comment or a name in `«…»` that is not closed, and a string or such a name that runs over a line break (no string gaps): what is counted must be what Lean reads.".to_string(),
                format!(
                    "An added file without code, {}a path of an added file that is longer than {} characters or holds anything but ASCII letters, digits, `_`, `/` and `.`.",
                    match cfg.limits.max_added_files {
                        0 => String::new(),
                        n => format!("more than {n} added files in one change, "),
                    },
                    cfg.limits.max_path_length
                ),
            ]),
    );
    let review = &cfg.review;
    let gates = bullets(
        gate_table(cfg).into_iter().filter(|g| g.0 != "review").map(|(_, label, description)| format!("{label}. {description}")),
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
    let vars: [(&str, String); 31] = [
        ("gates", gates),
        ("approvals_needed", match (review.required, review.required_large) {
            (0, large) => format!(
                "In this arena only a change of more than {} files needs approval ({large} of them); such a change merges on yours",
                review.large_change_files
            ),
            (1, large) if large > 1 => format!(
                "One approval from an account other than the author's merges a change (large changes need {large})"
            ),
            (1, _) => "One approval from an account other than the author's merges a change".to_string(),
            (n, large) if large > n => format!(
                "{n} approvals from accounts other than the author's merge a change (large changes need {large})"
            ),
            (n, _) => format!("{n} approvals from accounts other than the author's merge a change"),
        }),
        ("trial_rule", match cfg.ideas.trial_above {
            0 => "This arena does not require a **Trial** section, whatever the estimate; add one whenever you measured something.".to_string(),
            limit => format!("A fifth section, **Trial**, is required when the estimate is large: if the largest number in **Estimated saving** is above {limit} tokens, say in **Trial** what you ported or measured on one file or case and the saving you measured there. Without it the idea does not enter the queue."),
        }),
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
        ("large_change_files", cfg.review.large_change_files.to_string()),
        ("title_format", cfg.format.title_help()),
        (
            "techniques",
            bullets(cfg.format.techniques.iter().map(|t| format!("`{}`: {}", t.key, t.description))),
        ),
        ("protect_contributor", {
            let b = protect_bullets(cfg);
            if b.is_empty() {
                "Nothing else is protected. Whatever those theorems do not need may be deleted, and the statement of any intermediate result may change, as long as they stay proved.".to_string()
            } else {
                format!("Protected, may be re-proved but never removed:\n{b}")
            }
        }),
        ("protect_ideator", {
            let b = protect_bullets(cfg);
            if b.is_empty() {
                "Nothing besides those files and the final theorems is protected: a declaration, a clause of a statement or a whole file that the final theorems do not need is a valid target.".to_string()
            } else {
                format!("These declarations are protected and cannot be removed, so do not propose removing them:\n{b}")
            }
        }),
        ("protect_reviewer", {
            if cfg.protect.is_empty() {
                String::new()
            } else {
                "\n- A protected statement whose meaning changed: the gate compares its text, but a changed definition it mentions changes what it says.".to_string()
            }
        }),
        (
            "bootstrap_note",
            if cfg.roles.bootstrap {
                "\n\nBootstrap phase: the leaders currently run every agent, including you, from the project's own account, so the pull request may show that account as its author. That is expected. What matters is that you did not write the change and judge it independently; never approve work you produced yourself.".to_string()
            } else {
                String::new()
            },
        ),
        ("conduct_contributor", {
            let c = &cfg.conduct;
            let rules: Vec<String> = [c.time_rule(&p.branch), c.search_rule(), c.local_rule(), c.tidy_rule()]
                .into_iter()
                .filter(|r| !r.is_empty())
                .collect();
            if rules.is_empty() { String::new() } else { format!("\n\n{}", rules.join("\n\n")) }
        }),
        ("conduct_reviewer", cfg.conduct.reviewer_bullets()),
        ("max_open_prs", cfg.limits.max_open_prs.to_string()),
        ("reviewer_min_merged", cfg.roles.reviewer_min_merged.to_string()),
        ("ideator_min_merged", cfg.roles.ideator_min_merged.to_string()),
    ];
    let mut out = conditional(template, "review", review.on());
    out = conditional(&out, "large_review", review.required_large > review.required);
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
        "coordinator" => COORDINATOR,
        other => anyhow::bail!("unknown role `{other}`"),
    };
    if role == "reviewer" && !cfg.review.on() {
        return Ok(no_review(cfg));
    }
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

    // The settings the site shows. Where no change needs an approval, the
    // numbers about approvals are left out and one line says so.
    let review = &cfg.review;
    let conduct = &cfg.conduct;
    let on_off = |on: bool| if on { "checked" } else { "not checked" };
    let mut settings = serde_json::Map::new();
    let mut set = |k: &str, v: serde_json::Value| {
        settings.insert(k.to_string(), v);
    };
    set("Primary metric", json!(cfg.score.primary));
    set("Minimum improvement", json!(cfg.score.min_delta));
    set("Maximum line length", json!(cfg.score.max_line_length));
    if review.on() {
        set("Approvals required", if review.required == 0 { json!("none for a change that is not large") } else { json!(review.required) });
        set("Approvals for large changes", json!(review.required_large));
        set("Large change threshold (files)", json!(review.large_change_files));
    } else {
        set("Approvals required", json!("none: a change merges when its gates pass"));
    }
    set("Open changes per author", json!(cfg.limits.max_open_prs));
    set(
        "Added comments and string literals per change (max characters)",
        match cfg.limits.max_added_comment_chars {
            0 => json!("no limit"),
            n => json!(n),
        },
    );
    set("Minimum account age to apply for a role (days)", json!(cfg.roles.min_account_age_days));
    set("Idea claim length (hours)", json!(cfg.ideas.lease_hours));
    set("Compile time of a changed file (max % slower, not checked by a gate)", json!(conduct.max_slowdown_pct));
    set(
        "Search tactics to shorten proofs",
        if conduct.search_tactics.enabled {
            json!(format!("allowed under conditions, at most {} in a file", conduct.search_tactics.max_per_file))
        } else {
            json!("not allowed")
        },
    );
    set(
        "Local simp attributes",
        if conduct.local_automation.enabled {
            json!(format!("allowed under conditions, at most {} in a file", conduct.local_automation.max_local_simp_per_file))
        } else {
            json!("not allowed")
        },
    );
    set("Comment on an added clear", json!(on_off(conduct.comment_clear)));
    set("Comment on a switched-off simp lemma", json!(on_off(conduct.comment_disabled_lemmas)));
    set("No added simp only []", json!(on_off(conduct.no_empty_simp_only)));

    let rules = json!({
        "gates": gate_table(cfg).into_iter().map(|g| json!({"key": g.0, "label": g.1, "description": g.2})).collect::<Vec<_>>(),
        "review": {
            "on": review.on(),
            "required": review.required,
            "required_large": review.required_large,
            "large_change_files": review.large_change_files,
        },
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
            {"command": "/approve <commit>", "who": "reviewers and leaders", "where": "a pull request", "does": if review.on() {
                "Approve exactly that commit, named by its full hash, after the bot's first comment on it."
            } else {
                "Withdraw your own request for changes, for exactly that commit, named by its full hash. No change needs an approval here."
            }},
            {"command": "/request-changes <reason>", "who": "reviewers and leaders", "where": "a pull request", "does": if review.on() {
                "Block the change until the same reviewer approves."
            } else {
                "Stop a change that passes the gates: it does not merge until the same person approves its head commit."
            }},
            {"command": "/waive-min <commit> <reason>", "who": "leaders", "where": "a pull request", "does": "Accept a saving below the minimum for exactly that commit; it must still lower the score by at least 1."},
            {"command": "/accept", "who": "ideators and leaders", "where": "a proposed idea", "does": "Put the idea into the queue."},
            {"command": "/decline", "who": "ideators and leaders", "where": "a proposed idea or an application", "does": "Close it; give the reason in the same comment."},
            {"command": if review.on() { "/apply reviewer|ideator|leader" } else { "/apply ideator|leader" }, "who": "anyone with enough merged changes", "where": "any issue", "does": "Open an application for leaders to judge."},
            {"command": "/endorse", "who": "leaders", "where": "an application", "does": "Support the application."},
            {"command": "/grant @login reviewer|ideator", "who": "leaders", "where": "any issue", "does": "Give a role directly."},
            {"command": "/revoke @login <role>", "who": "leaders", "where": "any issue", "does": "Take a role away."},
        ],
        "settings": settings,
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

fn protect_bullets(cfg: &Config) -> String {
    bullets(cfg.protect.iter().map(|p| if p.description.is_empty() { p.name.clone() } else { p.description.clone() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(review: &str) -> Config {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "hc-render-cfg-{}-{}.toml",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let text = format!(
            "[project]\nslug = \"t\"\ntitle = \"T\"\nrepo = \"o/r\"\ndir = \"proj\"\nharness_repo = \"o/h\"\nfrozen = [\"proj/Statement.lean\"]\nlocal_build = [\"lake build\"]\ntheorems = [\"T.main\"]\n{review}"
        );
        std::fs::write(&path, text).unwrap();
        let cfg = Config::load(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        cfg
    }

    #[test]
    fn conditional_passages() {
        let t = "a {{#x}}on{{/x}}{{^x}}off{{/x}} b\n{{#x}}\nline for on\n{{/x}}\n{{^x}}\nline for off\n{{/x}}\nend {{#y}}kept{{/y}}\n";
        assert_eq!(conditional(t, "x", true), "a on b\nline for on\nend {{#y}}kept{{/y}}\n");
        assert_eq!(conditional(t, "x", false), "a off b\nline for off\nend {{#y}}kept{{/y}}\n");
        assert_eq!(conditional("1.\n{{#x}}\n2.\n{{/x}}\n3.\n", "x", false), "1.\n3.\n");
    }

    /// Every placeholder and every conditional marker is resolved, in every
    /// prompt, under every kind of review setting.
    #[test]
    fn prompts_render_completely() {
        for review in [
            "",
            "[review]\nrequired = 0\nrequired_large = 0\n",
            "[review]\nrequired = 1\nrequired_large = 2\n",
            "[review]\nrequired = 0\nrequired_large = 1\n",
        ] {
            let c = cfg(review);
            for role in ["contributor", "reviewer", "ideator", "leader", "coordinator"] {
                let text = role_prompt(role, &c, "https://site/").unwrap();
                assert!(!text.contains("{{") && !text.contains("}}"), "{role} with `{review}`: {text}");
                assert!(!text.contains("\n\n\n"), "{role} with `{review}` has an empty paragraph: {text}");
            }
        }
    }

    #[test]
    fn prompts_follow_the_review_setting() {
        let off = cfg("[review]\nrequired = 0\nrequired_large = 0\n");
        let on = cfg("[review]\nrequired = 1\nrequired_large = 2\n");
        // A file that says nothing about review is an arena with review.
        assert!(cfg("").review.on() && cfg("").review_unstated);
        let prompt = |role: &str, c: &Config| role_prompt(role, c, "https://site/").unwrap();

        // Contributor: merges once the gates pass, and what the gates check.
        let text = prompt("contributor", &off);
        assert!(text.contains("Once every gate passes, the change merges on its own: nobody has to approve it."), "{text}");
        assert!(text.contains("What the gates check"));
        for g in GATES.iter().filter(|g| g.0 != "review") {
            assert!(text.contains(&format!("- {}. ", g.1)), "the gate `{}` is listed", g.0);
        }
        assert!(text.contains("`/request-changes`"));
        assert!(!text.contains("/apply reviewer") && !text.contains("vetted reviewer must approve"), "{text}");
        assert!(!text.contains("Independent review"));
        let text = prompt("contributor", &on);
        assert!(text.contains("a vetted reviewer must approve") && text.contains("/apply reviewer"), "{text}");
        assert!(!text.contains("What the gates check"));

        // Reviewer: a short statement where there is nothing to review.
        let text = prompt("reviewer", &off);
        assert!(text.starts_with("This arena does not use review"), "{text}");
        assert!(text.contains("`/request-changes`") && text.lines().count() < 10, "{text}");
        assert!(prompt("reviewer", &on).contains("One approval from an account other than the author's merges a change (large changes need 2)"));
        let large_only = cfg("[review]\nrequired = 0\nrequired_large = 1\n");
        assert!(prompt("reviewer", &large_only).contains("only a change of more than 20 files needs approval"));

        // Coordinator: no reviewers are started.
        let text = prompt("coordinator", &off);
        assert!(text.contains("## Moving a change to merge") && text.contains("No reviewer is started"), "{text}");
        assert!(!text.contains("Start a reviewer") && !text.contains("`reviewer`"), "{text}");
        assert!(text.contains("Every change costs one full build"));
        let text = prompt("coordinator", &on);
        assert!(text.contains("## Moving a change through review") && text.contains("Start a reviewer for every new pull request"));
        assert!(text.contains("above that it needs 2 reviewers"));

        // Leader: the brake is stated in both.
        let text = prompt("leader", &off);
        assert!(text.contains("you are the only one who can stop one that passes the gates"), "{text}");
        assert!(text.contains("until you comment `/approve <commit>`") && !text.contains("Review queue"), "{text}");
        let text = prompt("leader", &on);
        assert!(text.contains("Review queue") && text.contains("A request for changes blocks the change"), "{text}");

        assert!(!prompt("ideator", &off).contains("reviewer"));
        assert!(prompt("ideator", &on).contains("a reviewer's careful reading"));
    }

    #[test]
    fn rules_follow_the_review_setting() {
        let dir = std::env::temp_dir().join(format!("hc-render-test-{}", std::process::id()));
        let read = |c: &Config| {
            let _ = std::fs::remove_dir_all(&dir);
            site_files(c, &dir, "https://site").unwrap();
            let v: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(dir.join("rules.json")).unwrap()).unwrap();
            let _ = std::fs::remove_dir_all(&dir);
            v
        };
        let keys = |v: &serde_json::Value| {
            v["gates"].as_array().unwrap().iter().map(|g| g["key"].as_str().unwrap().to_string()).collect::<Vec<_>>()
        };
        let commands = |v: &serde_json::Value| serde_json::to_string(&v["commands"]).unwrap();

        let off = read(&cfg("[review]\nrequired = 0\nrequired_large = 0\n"));
        assert_eq!(off["review"]["on"], false);
        assert!(!keys(&off).contains(&"review".to_string()));
        for key in hc_core::model::STATIC_GATES_2 {
            assert!(keys(&off).contains(&key.to_string()), "{key}");
        }
        assert_eq!(off["settings"]["Approvals required"], "none: a change merges when its gates pass");
        assert!(off["settings"].get("Approvals for large changes").is_none());
        assert!(!commands(&off).contains("/apply reviewer") && commands(&off).contains("/apply ideator|leader"));
        assert!(commands(&off).contains("Stop a change that passes the gates"));

        let on = read(&cfg("[review]\nrequired = 1\nrequired_large = 2\n"));
        assert_eq!(on["review"]["on"], true);
        assert_eq!(on["review"]["required"], 1);
        assert_eq!(keys(&on).last().map(String::as_str), Some("review"));
        assert_eq!(on["settings"]["Approvals required"], 1);
        assert_eq!(on["settings"]["Approvals for large changes"], 2);
        assert!(commands(&on).contains("/apply reviewer|ideator|leader"));
    }
}
