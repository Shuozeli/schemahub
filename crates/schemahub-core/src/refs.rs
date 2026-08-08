//! Bookmark / tag / merge orchestration (design.md §12). Thin authorized
//! wrappers over the JJ layer. Unprotected targets retain first-class merge
//! conflicts; protected targets reject a conflicted exact final tree before
//! publication (design.md §6).

use schemahub_jj::{NamedRefPage, PublicationError, RefSpec};
use schemahub_types::Action;

use crate::auth::authorize;
use crate::error::{CoreError, CoreResult};
use crate::mutation::idempotency::{FingerprintBuilder, IdempotentWrite};
use crate::request::MutationResponse;
use crate::Core;

impl Core {
    /// Create a bookmark at the commit `from` resolves to.
    pub fn create_bookmark(
        &self,
        project: &str,
        repo: &str,
        name: &str,
        from: &RefSpec,
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
        let protected = schemahub_jj::bookmark::is_protected(name, &config.protected_bookmarks);
        match self
            .jj
            .create_bookmark_validated(project, repo, name, from, author, |snapshot| {
                self.validate_publication_snapshot(project, repo, name, protected, snapshot)
            }) {
            Ok(commit) => Ok(commit),
            Err(PublicationError::Jj(error)) => Err(error.into()),
            Err(PublicationError::Rejected(error)) => Err(error),
        }
    }

    /// Move a bookmark to the commit `to` resolves to.
    pub fn move_bookmark(
        &self,
        project: &str,
        repo: &str,
        name: &str,
        to: &RefSpec,
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
        let protected = schemahub_jj::bookmark::is_protected(name, &config.protected_bookmarks);
        match self
            .jj
            .move_bookmark_validated(project, repo, name, to, author, |snapshot| {
                self.validate_publication_snapshot(project, repo, name, protected, snapshot)
            }) {
            Ok(commit) => Ok(commit),
            Err(PublicationError::Jj(error)) => Err(error.into()),
            Err(PublicationError::Rejected(error)) => Err(error),
        }
    }

    /// Delete a bookmark (branch). Requires `ManageRepo` — removing a branch is
    /// a repo-management action, not a routine write.
    pub fn delete_bookmark(
        &self,
        project: &str,
        repo: &str,
        name: &str,
        author: &str,
        token: Option<&str>,
    ) -> CoreResult<()> {
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            Action::ManageRepo,
            project,
            repo,
        )?;
        self.effective_repo_config(project, repo)?;
        Ok(self.jj.delete_bookmark(project, repo, name, author)?)
    }

    /// List bookmarks (name → target commit ids).
    pub fn list_bookmarks(
        &self,
        project: &str,
        repo: &str,
        token: Option<&str>,
    ) -> CoreResult<Vec<(String, Vec<String>)>> {
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            Action::Read,
            project,
            repo,
        )?;
        Ok(self.jj.list_bookmarks(project, repo)?)
    }

    /// Look up one bookmark without enumerating the complete namespace.
    pub fn get_bookmark(
        &self,
        project: &str,
        repo: &str,
        name: &str,
        token: Option<&str>,
    ) -> CoreResult<Option<String>> {
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            Action::Read,
            project,
            repo,
        )?;
        validate_ref_name("branch name", name)?;
        Ok(self.jj.get_bookmark(project, repo, name)?)
    }

    /// List a bounded bookmark page in stable lexicographical name order.
    pub fn list_bookmarks_page(
        &self,
        project: &str,
        repo: &str,
        name_prefix: &str,
        start_after: Option<&str>,
        limit: usize,
        token: Option<&str>,
    ) -> CoreResult<NamedRefPage> {
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            Action::Read,
            project,
            repo,
        )?;
        validate_ref_page(name_prefix, start_after, limit)?;
        Ok(self
            .jj
            .list_bookmarks_page(project, repo, name_prefix, start_after, limit)?)
    }

    /// Create a tag (immutable name → commit pin).
    pub fn create_tag(
        &self,
        project: &str,
        repo: &str,
        name: &str,
        at: &RefSpec,
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
        self.effective_repo_config(project, repo)?;
        Ok(self.jj.create_tag(project, repo, name, at, author)?)
    }

    /// Delete a tag. Requires `Force` (Maintainer+) — tags are immutable pins
    /// (design.md §6.3, §12), so retracting one is a privileged "force"-class
    /// action even though it touches no commits.
    pub fn delete_tag(
        &self,
        project: &str,
        repo: &str,
        name: &str,
        author: &str,
        token: Option<&str>,
    ) -> CoreResult<()> {
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            Action::Force,
            project,
            repo,
        )?;
        self.effective_repo_config(project, repo)?;
        Ok(self.jj.delete_tag(project, repo, name, author)?)
    }

    /// List tags (name → commit id).
    pub fn list_tags(
        &self,
        project: &str,
        repo: &str,
        token: Option<&str>,
    ) -> CoreResult<Vec<(String, String)>> {
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            Action::Read,
            project,
            repo,
        )?;
        Ok(self.jj.list_tags(project, repo)?)
    }

    /// List a bounded tag page in stable lexicographical name order.
    pub fn list_tags_page(
        &self,
        project: &str,
        repo: &str,
        name_prefix: &str,
        start_after: Option<&str>,
        limit: usize,
        token: Option<&str>,
    ) -> CoreResult<NamedRefPage> {
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            Action::Read,
            project,
            repo,
        )?;
        validate_ref_page(name_prefix, start_after, limit)?;
        Ok(self
            .jj
            .list_tags_page(project, repo, name_prefix, start_after, limit)?)
    }

    /// Merge bookmark `src` into `dst` (design.md §6). Conflicts become stored
    /// objects on unprotected targets; protected targets reject them atomically.
    #[allow(clippy::too_many_arguments)]
    pub fn merge(
        &self,
        project: &str,
        repo: &str,
        src: &str,
        dst: &str,
        base_revision: Option<&str>,
        author: &str,
        token: Option<&str>,
    ) -> CoreResult<MutationResponse> {
        self.merge_idempotent(
            project,
            repo,
            src,
            dst,
            base_revision,
            None,
            None,
            author,
            token,
        )
    }

    /// Merge with an optional durable idempotency receipt and caller-supplied
    /// commit message. Retries reconcile against the historical JJ operation.
    #[allow(clippy::too_many_arguments)]
    pub fn merge_idempotent(
        &self,
        project: &str,
        repo: &str,
        src: &str,
        dst: &str,
        base_revision: Option<&str>,
        idempotency_key: Option<&str>,
        message: Option<&str>,
        author: &str,
        token: Option<&str>,
    ) -> CoreResult<MutationResponse> {
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            Action::Write,
            project,
            repo,
        )?;
        let default_message = format!("merge {src} into {dst}");
        let message = message
            .filter(|value| !value.is_empty())
            .unwrap_or(&default_message);
        let mut fingerprint = FingerprintBuilder::new("merge");
        for field in [
            project.as_bytes(),
            repo.as_bytes(),
            src.as_bytes(),
            dst.as_bytes(),
            base_revision.unwrap_or_default().as_bytes(),
            message.as_bytes(),
            author.as_bytes(),
        ] {
            fingerprint.update(field);
        }
        let fingerprint = fingerprint.finish();
        let scope = format!("merge/{project}/{repo}");
        if let Some(response) =
            self.replay_idempotent_write(&scope, idempotency_key, &fingerprint, project, repo, dst)?
        {
            return Ok(response);
        }
        self.validate_base_revision(project, repo, base_revision)?;
        self.ensure_direct_write_allowed(project, repo)?;
        let config = self.effective_repo_config(project, repo)?;
        let protected = schemahub_jj::bookmark::is_protected(dst, &config.protected_bookmarks);
        let attempt = match self.begin_idempotent_write(
            &scope,
            idempotency_key,
            &fingerprint,
            project,
            repo,
            dst,
        )? {
            IdempotentWrite::Replay(response) => return Ok(response),
            IdempotentWrite::Proceed(attempt) => attempt,
        };
        let attributes = attempt
            .as_ref()
            .map(|attempt| attempt.attributes())
            .unwrap_or_default();
        let write = match self.jj.merge_with_attributes_validated(
            project,
            repo,
            src,
            dst,
            author,
            message,
            attributes,
            |snapshot| self.validate_publication_snapshot(project, repo, dst, protected, snapshot),
        ) {
            Ok(write) => write,
            Err(PublicationError::Jj(error)) => return Err(error.into()),
            Err(PublicationError::Rejected(error)) => {
                self.abort_idempotent_write(attempt.as_ref())?;
                return Err(error);
            }
        };
        let response = MutationResponse {
            commit_id: write.commit_id,
            change_id: write.change_id,
            conflicted_decls: write.conflicted_decls,
        };
        self.complete_idempotent_write(attempt.as_ref(), response)
    }
}

fn validate_ref_page(name_prefix: &str, start_after: Option<&str>, limit: usize) -> CoreResult<()> {
    if name_prefix.len() > 255 || name_prefix.chars().any(char::is_control) {
        return Err(CoreError::InvalidArgument(
            "name_prefix must be at most 255 characters without control characters".to_string(),
        ));
    }
    if limit == 0 || limit > 200 {
        return Err(CoreError::InvalidArgument(
            "ref page limit must be between 1 and 200".to_string(),
        ));
    }
    if let Some(cursor) = start_after {
        validate_ref_name("ref page cursor", cursor)?;
        if !cursor.starts_with(name_prefix) {
            return Err(CoreError::InvalidArgument(
                "ref page cursor must match name_prefix".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_ref_name(label: &str, value: &str) -> CoreResult<()> {
    if value.trim().is_empty() || value.len() > 255 || value.chars().any(char::is_control) {
        return Err(CoreError::InvalidArgument(format!(
            "{label} must be a non-empty value of at most 255 characters without control characters"
        )));
    }
    Ok(())
}
