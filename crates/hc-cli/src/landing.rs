//! The git tree main will have once a stack of changes, and then one more,
//! have landed.
//!
//! Verification applies the changes ahead of a pull request as real merges
//! and builds the result. On GitHub each of them lands as one squashed commit
//! that does not have the change's own commits as ancestors. For a pull
//! request that is independent of the changes ahead, both ways give the same
//! tree. For one whose branch contains commits of a change ahead of it (a
//! change built on another, `Depends: #n`), merging it after that change has
//! been squashed is a different three-way merge, from an older common
//! ancestor: it can conflict, and it can succeed with another tree than the
//! one that was built. So the workflow computes the tree both ways, and the
//! gatekeeper merges from a stacked result only when they agree.

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

/// What is reported instead of a tree when the sequence does not merge cleanly.
pub const CONFLICT: &str = "conflict";

/// Run git on content that is not trusted: no hooks, no filters, no signing.
fn git(dir: &Path, args: &[&str]) -> Result<(bool, String)> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c", "user.name=hillclimb",
            "-c", "user.email=hillclimb@users.noreply.github.com",
            "-c", "core.hooksPath=/dev/null",
            "-c", "filter.lfs.smudge=",
            "-c", "filter.lfs.process=",
            "-c", "filter.lfs.required=false",
            "-c", "commit.gpgsign=false",
        ])
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .context("running git")?;
    Ok((out.status.success(), String::from_utf8_lossy(&out.stdout).trim().to_string()))
}

/// The tree after `stack` (commits, in order) and then `head` have landed on
/// `base` the way GitHub lands them: each change in the stack as one commit
/// whose only parent is the main branch before it, and then the merge of
/// `head` into that. Returns [`CONFLICT`] if a step does not merge cleanly.
/// Works in a temporary worktree of `repo` and leaves `repo` as it was.
pub fn landing_tree(repo: &Path, base: &str, stack: &[String], head: &str) -> Result<String> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!("hc-landing-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&dir);
    let at = dir.to_string_lossy().to_string();
    if !git(repo, &["worktree", "add", "--quiet", "--detach", &at, base])?.0 {
        bail!("could not check out `{base}` to compute the tree as it will land");
    }
    let result = (|| {
        for commit in stack {
            if !git(&dir, &["merge", "--quiet", "--squash", commit])?.0 {
                return Ok(CONFLICT.to_string());
            }
            if !git(&dir, &["commit", "--quiet", "--allow-empty", "--no-verify", "-m", "squash"])?.0 {
                bail!("could not record the squashed commit of {commit}");
            }
        }
        if !git(&dir, &["merge", "--quiet", "--no-ff", "--no-edit", head])?.0 {
            return Ok(CONFLICT.to_string());
        }
        let (ok, tree) = git(&dir, &["rev-parse", "HEAD^{tree}"])?;
        if !ok || tree.is_empty() {
            bail!("could not read the tree as it will land");
        }
        Ok(tree)
    })();
    let _ = git(repo, &["worktree", "remove", "--force", &at]);
    let _ = std::fs::remove_dir_all(&dir);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A repository with a main branch and a way to make branches off any commit.
    struct Repo(PathBuf);

    impl Repo {
        fn new(name: &str) -> Repo {
            let dir = std::env::temp_dir().join(format!("hc-landing-test-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let r = Repo(dir);
            r.run(&["init", "--quiet", "-b", "main"]);
            r.write("f.txt", "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\n");
            r.write("g.txt", "alpha\n");
            r.commit("base")
        }
        fn run(&self, args: &[&str]) -> String {
            let (ok, out) = git(&self.0, args).unwrap();
            assert!(ok, "git {args:?} failed");
            out
        }
        fn write(&self, file: &str, text: &str) {
            std::fs::write(self.0.join(file), text).unwrap();
        }
        fn commit(self, message: &str) -> Repo {
            self.run(&["add", "-A"]);
            self.run(&["commit", "--quiet", "-m", message]);
            self
        }
        /// A branch off `from` with one commit that writes `file`; returns its head.
        fn branch(&self, name: &str, from: &str, file: &str, text: &str) -> String {
            self.run(&["checkout", "--quiet", "-b", name, from]);
            self.write(file, text);
            self.run(&["add", "-A"]);
            self.run(&["commit", "--quiet", "-m", name]);
            self.run(&["rev-parse", "HEAD"])
        }
        /// The tree verification builds: `stack` and `head` applied as real merges.
        fn built_tree(&self, base: &str, stack: &[String], head: &str) -> String {
            self.run(&["checkout", "--quiet", "--detach", base]);
            for commit in stack.iter().map(String::as_str).chain([head]) {
                self.run(&["merge", "--quiet", "--no-ff", "--no-edit", commit]);
            }
            self.run(&["rev-parse", "HEAD^{tree}"])
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const BASE: &str = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\n";

    #[test]
    fn an_independent_change_lands_as_the_tree_that_was_built() {
        let r = Repo::new("independent");
        let main = r.run(&["rev-parse", "main"]);
        let first = r.branch("first", "main", "f.txt", &BASE.replace("two", "TWO"));
        let other = r.branch("other", "main", "g.txt", "beta\n");
        let stack = vec![first];
        let landing = landing_tree(&r.0, &main, &stack, &other).unwrap();
        assert_eq!(landing, r.built_tree(&main, &stack, &other));
        // With nothing ahead it is the plain merge.
        assert_eq!(landing_tree(&r.0, &main, &[], &other).unwrap(), r.built_tree(&main, &[], &other));
        // The repository is left alone: no extra worktree stays behind.
        assert_eq!(r.run(&["worktree", "list"]).lines().count(), 1);
    }

    #[test]
    fn a_change_built_on_another_lands_as_built_when_it_edits_other_places() {
        let r = Repo::new("apart");
        let main = r.run(&["rev-parse", "main"]);
        let first = r.branch("first", "main", "f.txt", &BASE.replace("two", "TWO"));
        // Built on `first`: its branch contains that commit, and it edits
        // another file and a distant line of the same file.
        r.branch("second-a", "first", "g.txt", "beta\n");
        let second = r.branch("second", "second-a", "f.txt", &BASE.replace("two", "TWO").replace("seven", "SEVEN"));
        let stack = vec![first];
        let landing = landing_tree(&r.0, &main, &stack, &second).unwrap();
        assert_ne!(landing, CONFLICT);
        assert_eq!(landing, r.built_tree(&main, &stack, &second));
    }

    #[test]
    fn a_change_built_on_another_conflicts_once_that_is_squashed_if_it_edits_the_same_lines() {
        let r = Repo::new("overlap");
        let main = r.run(&["rev-parse", "main"]);
        let first = r.branch("first", "main", "f.txt", &BASE.replace("two", "TWO"));
        let second = r.branch("second", "first", "f.txt", &BASE.replace("two", "2"));
        let stack = vec![first];
        // As real merges it applies cleanly, which is what was built.
        r.built_tree(&main, &stack, &second);
        // After the squash the common ancestor is main, and both sides changed the line.
        assert_eq!(landing_tree(&r.0, &main, &stack, &second).unwrap(), CONFLICT);
    }

    #[test]
    fn a_change_that_undoes_part_of_what_it_is_built_on_lands_as_another_tree_without_a_conflict() {
        let r = Repo::new("undo");
        let main = r.run(&["rev-parse", "main"]);
        let first = r.branch("first", "main", "f.txt", &BASE.replace("two", "TWO").replace("seven", "SEVEN"));
        // Built on `first`, it puts one of its lines back as it was on main.
        let second = r.branch("second", "first", "f.txt", &BASE.replace("seven", "SEVEN"));
        let stack = vec![first];
        let built = r.built_tree(&main, &stack, &second);
        let landing = landing_tree(&r.0, &main, &stack, &second).unwrap();
        // Seen from main the branch never touched that line, so the merge
        // keeps main's (squashed) version: a clean merge to a tree nobody built.
        assert_ne!(landing, CONFLICT);
        assert_ne!(landing, built, "only comparing the trees catches this");
    }
}
