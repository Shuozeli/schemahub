//! History & recovery (design.md §12): the commit/change log, the operation
//! log (audit), undo, and per-declaration diff between two refs.

use std::collections::BTreeSet;

use schemahub_jj::RefSpec;
use schemahub_types::{Action, DeclChange, SchemaPath};

use crate::auth::authorize;
use crate::error::CoreResult;
use crate::request::{LogEntry, OperationRecord};
use crate::Core;

/// Default number of commits returned by [`Core::log`] when no limit is given.
const DEFAULT_LOG_LIMIT: usize = 100;

impl Core {
    /// The operation log for a repo (the audit record) — `jj.list_operations`.
    ///
    /// `limit = Some(n)` returns the latest `n` operations (the JJ layer returns
    /// oldest→newest, so we trim the front, preserving relative order). `None`
    /// returns the full log.
    pub fn op_log(
        &self,
        project: &str,
        repo: &str,
        limit: Option<usize>,
        token: Option<&str>,
    ) -> CoreResult<Vec<OperationRecord>> {
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            Action::Read,
            project,
            repo,
        )?;
        let mut ops = self.jj.list_operations(project, repo)?;
        if let Some(n) = limit {
            if ops.len() > n {
                let drop = ops.len() - n;
                ops.drain(..drop);
            }
        }
        Ok(ops)
    }

    /// Undo the last operation — `jj.undo`. Returns the id of the operation that
    /// was undone. Requires `Write` (it mutates the repo's view).
    pub fn undo(
        &self,
        project: &str,
        repo: &str,
        author: &str,
        token: Option<&str>,
    ) -> CoreResult<String> {
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            Action::Write,
            project,
            repo,
        )?;
        Ok(self.jj.undo(project, repo, author)?)
    }

    /// The commit/change history graph (design.md §12 `log`).
    ///
    /// Walks the *real* commit/change graph from `at_ref` (newest→oldest) via
    /// [`Jj::commit_log`], surfacing each commit's content-addressed `commit_id`,
    /// stable jj `change_id`, real `parents`, `author`, `message`, and
    /// `timestamp`. This is distinct from [`Core::op_log`], which is the
    /// operation-log audit view. Defaults to the repo's default bookmark when no
    /// ref is given.
    pub fn log(
        &self,
        project: &str,
        repo: &str,
        at_ref: Option<&RefSpec>,
        limit: Option<usize>,
        token: Option<&str>,
    ) -> CoreResult<Vec<LogEntry>> {
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            Action::Read,
            project,
            repo,
        )?;
        let default_ref = RefSpec::bookmark(self.repo_configs.get(project, repo).default_bookmark);
        let at = at_ref.unwrap_or(&default_ref);
        let limit = limit.unwrap_or(DEFAULT_LOG_LIMIT);
        let commits = self.jj.commit_log(project, repo, at, limit)?;
        Ok(commits
            .into_iter()
            .map(|c| LogEntry {
                commit_id: c.commit_id,
                change_id: c.change_id,
                parents: c.parents,
                author: c.author,
                message: c.message,
                timestamp: c.timestamp,
            })
            .collect())
    }

    /// Per-declaration diff of one schema file between two refs (design.md §12
    /// `diff`). Loads both sides and diffs every declaration present in either,
    /// via `compiler.diff_decl`.
    pub fn diff(
        &self,
        schema: &SchemaPath,
        from: &RefSpec,
        to: &RefSpec,
        token: Option<&str>,
    ) -> CoreResult<Vec<DeclChange>> {
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            Action::Read,
            &schema.project,
            &schema.repo,
        )?;
        let compiler = self.compiler_for(&schema.schema_name)?;
        let old = self
            .jj
            .load_schema(&schema.project, &schema.repo, &schema.schema_name, from)?;
        let new = self
            .jj
            .load_schema(&schema.project, &schema.repo, &schema.schema_name, to)?;

        let mut names: BTreeSet<&String> = BTreeSet::new();
        names.extend(old.decls.keys());
        names.extend(new.decls.keys());

        let mut changes = Vec::new();
        for name in names {
            match (old.decls.get(name), new.decls.get(name)) {
                (Some(o), Some(n)) => {
                    if o != n {
                        changes.push(compiler.diff_decl(o, n)?);
                    }
                }
                (None, Some(_)) => {
                    changes.push(DeclChange::DeclarationAdded { name: name.clone() })
                }
                (Some(_), None) => {
                    changes.push(DeclChange::DeclarationRemoved { name: name.clone() })
                }
                (None, None) => unreachable!("name came from the union of both key sets"),
            }
        }
        Ok(changes)
    }

    /// Diff between two bookmarks (convenience over [`Core::diff`]).
    pub fn diff_bookmarks(
        &self,
        schema: &SchemaPath,
        from: &str,
        to: &str,
        token: Option<&str>,
    ) -> CoreResult<Vec<DeclChange>> {
        self.diff(
            schema,
            &RefSpec::bookmark(from),
            &RefSpec::bookmark(to),
            token,
        )
    }
}
