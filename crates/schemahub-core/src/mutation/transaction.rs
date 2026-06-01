//! Transaction flow (design.md §5.2): an ordered batch under one commit / one
//! operation. Identical to the single flow except the compiler validates only
//! the *final* state per file (`apply_mutations`), every changed decl is
//! compat-checked, and limits are validated up front.
//!
//! ## Multi-file scope
//! A transaction may now span several schema files. The ordered ops are grouped
//! by their [`Mutation::schema_path`](schemahub_types::Mutation) (order preserved
//! within each file). Each touched file is loaded, its ops are applied through
//! the compiler to yield one [`MutationEffect`](schemahub_types::MutationEffect),
//! and every effect is committed atomically through
//! [`Vcs::commit_write_multi`](schemahub_vcs::Vcs::commit_write_multi) — one
//! commit, one operation. All ops in a transaction must still share a single
//! `format_id` (a transaction does not mix formats).

use schemahub_types::{Action, Mutation, MutationEffect};
use schemahub_vcs::RefSpec;

use crate::auth::authorize;
use crate::error::{CoreError, CoreResult};
use crate::mutation::{compat, load_base};
use crate::request::{MutationResponse, TransactionLimits, TransactionRequest};
use crate::Core;

impl Core {
    /// Transaction flow with default limits.
    pub fn apply_mutations(&self, req: TransactionRequest) -> CoreResult<MutationResponse> {
        self.apply_mutations_with_limits(req, TransactionLimits::default())
    }

    /// Transaction flow with explicit limits (design.md §5.2).
    pub fn apply_mutations_with_limits(
        &self,
        req: TransactionRequest,
        limits: TransactionLimits,
    ) -> CoreResult<MutationResponse> {
        // 1. Idempotency dedupe at the edge.
        if let Some(key) = &req.idempotency_key {
            if let Some(stored) = self.idempotency.get(key) {
                return Ok(stored);
            }
        }

        // Validate limits *before* doing any work (design.md §5.2).
        if req.mutations.is_empty() {
            return Err(CoreError::EmptyTransaction);
        }
        if req.mutations.len() > limits.max_ops {
            return Err(CoreError::LimitExceeded(format!(
                "{} ops > max {}",
                req.mutations.len(),
                limits.max_ops
            )));
        }

        // Validate the shared target: one (project, repo) + one format_id, and
        // group ops by schema file (preserving order within each file).
        let plan = TransactionPlan::build(&req.mutations, limits)?;

        // 2/3. Auth (one repo for the whole transaction).
        let action = if req.force {
            Action::Force
        } else {
            Action::Write
        };
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            req.token.as_deref(),
            action,
            &plan.project,
            &plan.repo,
        )?;

        let compiler = self
            .registry
            .get(&plan.format_id)
            .ok_or_else(|| CoreError::UnknownFormat(plan.format_id.clone()))?
            .clone();

        // 4/5/6. For each touched file: load its base, apply that file's ops (only
        // the final state is validated), and compat-check the change on protected
        // bookmarks. Collect one effect per file for the atomic commit.
        let base_ref = RefSpec::bookmark(req.bookmark.clone());
        let config = self.repo_configs.get(&plan.project, &plan.repo);
        let protected = !req.force
            && schemahub_vcs::bookmark::is_protected(&req.bookmark, &config.protected_bookmarks);

        let mut effects: Vec<(String, MutationEffect)> = Vec::with_capacity(plan.by_file.len());
        for (schema_name, ops) in &plan.by_file {
            let base = load_base(&self.vcs, &plan.project, &plan.repo, schema_name, &base_ref)?;
            let effect = compiler.apply_mutations(&base, ops)?;
            if protected {
                compat::gate(compiler.as_ref(), &config.compat_rules(), &base, &effect)?;
            }
            effects.push((schema_name.clone(), effect));
        }

        // 7. One commit / one operation across ALL touched files.
        let write = self.vcs.commit_write_multi(
            &plan.project,
            &plan.repo,
            &req.bookmark,
            &base_ref,
            effects,
            &req.author,
            &req.message,
        )?;

        let response = MutationResponse {
            commit_id: write.commit_id,
            change_id: write.change_id,
            conflicted_decls: write.conflicted_decls,
        };

        if let Some(key) = &req.idempotency_key {
            self.idempotency.put(key, response.clone());
        }

        Ok(response)
    }
}

/// The validated, grouped plan for a transaction: a single `(project, repo)` and
/// `format_id`, plus the ordered ops grouped by schema file. File order (first
/// appearance) and op order within each file are both preserved; the file count
/// is tiny (`max_schemas`), so a linear-probe `Vec` is preferable to pulling in
/// an ordered-map dependency.
struct TransactionPlan {
    project: String,
    repo: String,
    format_id: String,
    by_file: Vec<(String, Vec<Mutation>)>,
}

impl TransactionPlan {
    /// Validate that every op targets one `(project, repo)` + `format_id`, group
    /// ops by schema file, and enforce the schema-count limit.
    fn build(ops: &[Mutation], limits: TransactionLimits) -> CoreResult<Self> {
        let first = &ops[0].schema_path;
        let project = first.project.clone();
        let repo = first.repo.clone();
        let format_id = ops[0].format_id.clone();

        // All ops must share one format_id (a transaction never mixes formats).
        if ops.iter().any(|m| m.format_id != format_id) {
            return Err(CoreError::MixedTransaction(
                "all transaction ops must share one format_id".to_string(),
            ));
        }
        // All ops must target one (project, repo); a transaction is repo-scoped.
        if ops
            .iter()
            .any(|m| m.schema_path.project != project || m.schema_path.repo != repo)
        {
            return Err(CoreError::MixedTransaction(
                "all transaction ops must target one project/repo".to_string(),
            ));
        }

        // Group by schema file, preserving first-appearance file order and the
        // op order within each file.
        let mut by_file: Vec<(String, Vec<Mutation>)> = Vec::new();
        for m in ops {
            let name = &m.schema_path.schema_name;
            match by_file.iter_mut().find(|(n, _)| n == name) {
                Some((_, group)) => group.push(m.clone()),
                None => by_file.push((name.clone(), vec![m.clone()])),
            }
        }

        if by_file.len() > limits.max_schemas {
            return Err(CoreError::LimitExceeded(format!(
                "transaction touches {} schema files > max {}",
                by_file.len(),
                limits.max_schemas
            )));
        }

        Ok(Self {
            project,
            repo,
            format_id,
            by_file,
        })
    }
}
