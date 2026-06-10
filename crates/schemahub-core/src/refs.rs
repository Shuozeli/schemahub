//! Bookmark / tag / merge orchestration (design.md §12). Thin authorized
//! wrappers over the JJ layer; merge produces first-class conflicts, never errors
//! (design.md §6).

use schemahub_jj::RefSpec;
use schemahub_types::Action;

use crate::auth::authorize;
use crate::error::CoreResult;
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
        Ok(self.jj.create_bookmark(project, repo, name, from, author)?)
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
        Ok(self.jj.move_bookmark(project, repo, name, to, author)?)
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

    /// Merge bookmark `src` into `dst` (design.md §6). Conflicts become stored
    /// objects (reported in `conflicted_decls`), never an error.
    pub fn merge(
        &self,
        project: &str,
        repo: &str,
        src: &str,
        dst: &str,
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
        let write = self.jj.merge(project, repo, src, dst, author)?;
        Ok(MutationResponse {
            commit_id: write.commit_id,
            change_id: write.change_id,
            conflicted_decls: write.conflicted_decls,
        })
    }
}
