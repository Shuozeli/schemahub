//! History & recovery (design.md §12): the commit/change log, the operation
//! log (audit), undo, and per-declaration diff between two refs.

use std::collections::BTreeSet;

use schemahub_jj::{PublicationError, RefSpec};
use schemahub_types::{Action, DeclChange, SchemaPath};

use crate::auth::authorize;
use crate::error::CoreResult;
use crate::request::{LogEntry, OperationRecord, RepositoryDiff};
use crate::Core;

/// Default number of commits returned by [`Core::log`] when no limit is given.
const DEFAULT_LOG_LIMIT: usize = 100;

impl Core {
    /// The operation log for a repo (the audit record).
    ///
    /// `limit = Some(n)` asks JJ for only the latest `n` operations, preserving
    /// oldest→newest order. `None` returns the full log.
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
        match limit {
            Some(limit) => Ok(self.jj.list_operations_tail(project, repo, limit)?),
            None => Ok(self.jj.list_operations(project, repo)?),
        }
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
        let config = self.effective_repo_config(project, repo)?;
        match self
            .jj
            .undo_validated(project, repo, author, |bookmark, snapshot| {
                let protected =
                    schemahub_jj::bookmark::is_protected(bookmark, &config.protected_bookmarks);
                self.validate_publication_snapshot(project, repo, bookmark, protected, snapshot)
            }) {
            Ok(operation) => Ok(operation),
            Err(PublicationError::Jj(error)) => Err(error.into()),
            Err(PublicationError::Rejected(error)) => Err(error),
        }
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
        let (entries, _) = self.log_resolved(project, repo, at_ref, limit, token)?;
        Ok(entries)
    }

    /// Walk history from one immutable, repository-owned snapshot and return
    /// the exact commit used as the traversal root.
    pub fn log_resolved(
        &self,
        project: &str,
        repo: &str,
        at_ref: Option<&RefSpec>,
        limit: Option<usize>,
        token: Option<&str>,
    ) -> CoreResult<(Vec<LogEntry>, String)> {
        let at_commit = match at_ref {
            Some(at) => self.resolve_read_commit(project, repo, at, token)?,
            None => {
                let bookmark =
                    self.repository_default_bookmark(project, repo, Action::Read, token)?;
                self.resolve_read_commit(project, repo, &RefSpec::bookmark(bookmark), token)?
            }
        };
        let limit = limit.unwrap_or(DEFAULT_LOG_LIMIT);
        let commits =
            self.jj
                .commit_log(project, repo, &RefSpec::commit(at_commit.clone()), limit)?;
        let entries = commits
            .into_iter()
            .map(|c| LogEntry {
                commit_id: c.commit_id,
                change_id: c.change_id,
                parents: c.parents,
                author: c.author,
                message: c.message,
                timestamp: c.timestamp,
            })
            .collect();
        Ok((entries, at_commit))
    }

    /// Commit stream planning with optional exclusive stop and schema-path
    /// filtering. All commit coordinates are validated against the repository;
    /// an unreachable stop fails instead of silently returning an unbounded
    /// range that a caller could mistake for the requested interval.
    #[allow(clippy::too_many_arguments)]
    pub fn list_commits_resolved(
        &self,
        project: &str,
        repo: &str,
        at_ref: Option<&RefSpec>,
        stop_at_commit: Option<&str>,
        schema_name: Option<&str>,
        max_scanned: usize,
        token: Option<&str>,
    ) -> CoreResult<(Vec<LogEntry>, String)> {
        let fetch_limit = max_scanned.checked_add(1).ok_or_else(|| {
            crate::error::CoreError::InvalidArgument(
                "commit history scan limit is too large".to_string(),
            )
        })?;
        let (mut entries, at_commit) =
            self.log_resolved(project, repo, at_ref, Some(fetch_limit), token)?;
        let mut overflow = entries.len() > max_scanned;
        if let Some(stop) = stop_at_commit.filter(|stop| !stop.is_empty()) {
            let stop =
                self.resolve_read_commit(project, repo, &RefSpec::commit(stop.to_string()), token)?;
            let Some(position) = entries.iter().position(|entry| entry.commit_id == stop) else {
                if overflow {
                    return Err(crate::error::CoreError::ResourceExhausted(format!(
                        "commit history range exceeds the {max_scanned}-commit scan bound before stop {stop}"
                    )));
                }
                return Err(crate::error::CoreError::FailedPrecondition(format!(
                    "stop commit {stop} is not reachable from {at_commit} within the requested history range"
                )));
            };
            entries.truncate(position);
            overflow = false;
        }
        if overflow {
            return Err(crate::error::CoreError::ResourceExhausted(format!(
                "commit history exceeds the {max_scanned}-commit scan bound"
            )));
        }
        if let Some(schema_name) = schema_name.filter(|name| !name.is_empty()) {
            let mut filtered = Vec::new();
            for entry in entries {
                if self
                    .jj
                    .commit_touches_schema(project, repo, &entry.commit_id, schema_name)?
                {
                    filtered.push(entry);
                }
            }
            entries = filtered;
        }
        Ok((entries, at_commit))
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
        let diff = self.diff_repository_resolved(
            &schema.project,
            &schema.repo,
            Some(&schema.schema_name),
            from,
            to,
            token,
        )?;
        Ok(diff
            .schema_diffs
            .into_iter()
            .next()
            .map(|(_, changes)| changes)
            .unwrap_or_default())
    }

    /// Diff an entire repository or one schema between two immutable snapshots.
    /// Added/deleted schema files are represented as declaration additions or
    /// removals instead of failing one side's load.
    pub fn diff_repository_resolved(
        &self,
        project: &str,
        repo: &str,
        schema_name: Option<&str>,
        from: &RefSpec,
        to: &RefSpec,
        token: Option<&str>,
    ) -> CoreResult<RepositoryDiff> {
        let from_commit = self.resolve_read_commit(project, repo, from, token)?;
        let to_commit = self.resolve_read_commit(project, repo, to, token)?;
        let from_ref = RefSpec::commit(from_commit.clone());
        let to_ref = RefSpec::commit(to_commit.clone());
        let schema_names: BTreeSet<String> = match schema_name {
            Some(name) => BTreeSet::from([name.to_string()]),
            None => self
                .jj
                .list_schemas(project, repo, &from_ref)?
                .into_iter()
                .chain(self.jj.list_schemas(project, repo, &to_ref)?)
                .collect(),
        };

        let mut diffs = Vec::new();
        for schema_name in schema_names {
            let old = match self.jj.load_schema(project, repo, &schema_name, &from_ref) {
                Ok(objects) => Some(objects),
                Err(schemahub_jj::JjError::SchemaNotFound(_)) => None,
                Err(error) => return Err(error.into()),
            };
            let new = match self.jj.load_schema(project, repo, &schema_name, &to_ref) {
                Ok(objects) => Some(objects),
                Err(schemahub_jj::JjError::SchemaNotFound(_)) => None,
                Err(error) => return Err(error.into()),
            };
            if old.is_none() && new.is_none() {
                return Err(schemahub_jj::JjError::SchemaNotFound(schema_name).into());
            }
            let compiler = self.compiler_for(&schema_name)?;
            let mut declaration_names = BTreeSet::new();
            if let Some(old) = &old {
                declaration_names.extend(old.decls.keys().cloned());
            }
            if let Some(new) = &new {
                declaration_names.extend(new.decls.keys().cloned());
            }
            let mut changes = Vec::new();
            for name in declaration_names {
                let old_blob = old.as_ref().and_then(|objects| objects.decls.get(&name));
                let new_blob = new.as_ref().and_then(|objects| objects.decls.get(&name));
                match (old_blob, new_blob) {
                    (Some(old), Some(new)) if old != new => {
                        changes.push(compiler.diff_decl(old, new)?);
                    }
                    (None, Some(_)) => changes.push(DeclChange::DeclarationAdded { name }),
                    (Some(_), None) => changes.push(DeclChange::DeclarationRemoved { name }),
                    (Some(_), Some(_)) => {}
                    (None, None) => unreachable!("name came from a declaration union"),
                }
            }
            if !changes.is_empty() {
                diffs.push((schema_name, changes));
            }
        }
        Ok(RepositoryDiff {
            schema_diffs: diffs,
            base_commit: from_commit,
            head_commit: to_commit,
        })
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
