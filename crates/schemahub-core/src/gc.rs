//! Admin operations: GC and rebuild-index — thin wrappers over the JJ layer
//! (design.md §8 reference integrity index is derived/rebuildable; GC is the
//! mark-and-sweep over reachable objects, design.md §4.4).

use schemahub_types::Action;

use crate::auth::authorize;
use crate::error::CoreResult;
use crate::Core;

impl Core {
    /// Garbage-collect unreachable objects across the given `(project, repo)`
    /// roots. Returns the number of objects swept. Requires `ManageRepo`.
    pub fn gc(&self, repos: &[(String, String)], token: Option<&str>) -> CoreResult<usize> {
        // Authorize against the first repo (admin op). With more than one repo,
        // each must be authorized; for the common single-repo call this is exact.
        for (project, repo) in repos {
            authorize(
                self.authn.as_ref(),
                self.authz.as_ref(),
                token,
                Action::ManageRepo,
                project,
                repo,
            )?;
        }
        Ok(self.jj.gc(repos)?)
    }

    /// Rebuild the derived dependency/name index for a repo (design.md §8).
    ///
    /// The index is *derived* from objects reachable from bookmarks — there is no
    /// separately persisted index in the current JJ, so reads (Search /
    /// FollowType) recompute it on demand. This is therefore a no-op stub that
    /// authorizes and reports success; when a persisted index is added, the
    /// rebuild scan lands here. The server may expose it as `RebuildIndex`.
    pub fn rebuild_index(&self, project: &str, repo: &str, token: Option<&str>) -> CoreResult<()> {
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            Action::ManageRepo,
            project,
            repo,
        )?;
        // No persisted index today; reads are computed live. Nothing to rebuild.
        Ok(())
    }
}
