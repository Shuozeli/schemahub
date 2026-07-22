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
//! [`Jj::commit_write_multi`](schemahub_jj::Jj::commit_write_multi) — one
//! commit, one operation. All ops in a transaction must still share a single
//! `format_id` (a transaction does not mix formats).

use schemahub_jj::SchemaWrite;
use schemahub_types::{Action, Mutation, MutationEffect};

use crate::auth::authorize;
use crate::error::{CoreError, CoreResult};
use crate::mutation::idempotency::{force_audit_attributes, FingerprintBuilder};
use crate::mutation::{compat, immutable_bookmark_base, load_base};
use crate::request::{
    MutationResponse, TransactionDeadline, TransactionLimits, TransactionRequest,
};
use crate::Core;

impl Core {
    /// Transaction flow with default limits.
    pub fn apply_mutations(&self, req: TransactionRequest) -> CoreResult<MutationResponse> {
        self.apply_mutations_with_limits_and_deadline(req, TransactionLimits::default(), None)
    }

    /// Transaction flow bounded by a server-owned monotonic deadline.
    ///
    /// The deadline is checked throughout planning and again while JJ holds the
    /// repository publication guard. Cancellation therefore prevents work that
    /// outlives its RPC from starting a late publication.
    pub fn apply_mutations_with_deadline(
        &self,
        req: TransactionRequest,
        deadline: TransactionDeadline,
    ) -> CoreResult<MutationResponse> {
        self.apply_mutations_with_limits_and_deadline(
            req,
            TransactionLimits::default(),
            Some(&deadline),
        )
    }

    /// Transaction flow with explicit limits (design.md §5.2).
    pub fn apply_mutations_with_limits(
        &self,
        req: TransactionRequest,
        limits: TransactionLimits,
    ) -> CoreResult<MutationResponse> {
        self.apply_mutations_with_limits_and_deadline(req, limits, None)
    }

    fn apply_mutations_with_limits_and_deadline(
        &self,
        req: TransactionRequest,
        limits: TransactionLimits,
        deadline: Option<&TransactionDeadline>,
    ) -> CoreResult<MutationResponse> {
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
        ensure_before_deadline(deadline)?;

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
        ensure_before_deadline(deadline)?;
        let mut fingerprint = FingerprintBuilder::new("apply-transaction");
        for field in [
            plan.project.as_bytes(),
            plan.repo.as_bytes(),
            req.bookmark.as_bytes(),
            req.author.as_bytes(),
            req.message.as_bytes(),
            req.base_revision.as_deref().unwrap_or_default().as_bytes(),
        ] {
            fingerprint.update(field);
        }
        fingerprint.update(&[u8::from(req.force)]);
        for mutation in &req.mutations {
            fingerprint.update(mutation.schema_path.schema_name.as_bytes());
            fingerprint.update(mutation.format_id.as_bytes());
            fingerprint.update(mutation.operation.as_ref());
        }
        let fingerprint = fingerprint.finish();
        let scope = format!("apply-transaction/{}/{}", plan.project, plan.repo);
        if let Some(response) = self.replay_idempotent_write(
            &scope,
            req.idempotency_key.as_deref(),
            &fingerprint,
            &plan.project,
            &plan.repo,
            &req.bookmark,
        )? {
            return Ok(response);
        }
        self.validate_base_revision(&plan.project, &plan.repo, req.base_revision.as_deref())?;
        ensure_before_deadline(deadline)?;

        let compiler = self
            .registry
            .get(&plan.format_id)
            .ok_or_else(|| CoreError::UnknownFormat(plan.format_id.clone()))?
            .clone();

        // 4/5/6. For each touched file: load its base, apply that file's ops (only
        // the final state is validated), and compat-check the change on protected
        // bookmarks. Collect one effect per file for the atomic commit.
        let base_ref = immutable_bookmark_base(&self.jj, &plan.project, &plan.repo, &req.bookmark)?;
        self.ensure_direct_write_allowed(&plan.project, &plan.repo)?;
        let config = self.effective_repo_config(&plan.project, &plan.repo)?;
        let protected = !req.force
            && schemahub_jj::bookmark::is_protected(&req.bookmark, &config.protected_bookmarks);
        ensure_before_deadline(deadline)?;

        let mut effects: Vec<(String, MutationEffect)> = Vec::with_capacity(plan.by_file.len());
        for (schema_name, ops) in &plan.by_file {
            ensure_before_deadline(deadline)?;
            let base = load_base(&self.jj, &plan.project, &plan.repo, schema_name, &base_ref)?;
            let effect = compiler.apply_mutations(&base, ops)?;
            ensure_before_deadline(deadline)?;
            if protected {
                compat::gate(compiler.as_ref(), &config.compat_rules(), &base, &effect)?;
            }
            effects.push((schema_name.clone(), effect));
        }

        let writes = effects
            .into_iter()
            .map(|(schema_path, effect)| SchemaWrite::Patch {
                schema_path,
                effect,
            })
            .collect();
        ensure_before_deadline(deadline)?;

        // One commit / one operation across all touched files, correlated with
        // the durable receipt for safe restart replay.
        self.commit_idempotent_schema_changes_with_attributes_and_deadline(
            &scope,
            req.idempotency_key.as_deref(),
            &fingerprint,
            &plan.project,
            &plan.repo,
            &req.bookmark,
            &base_ref,
            writes,
            &req.author,
            &req.message,
            force_audit_attributes(req.force),
            deadline,
        )
    }
}

fn ensure_before_deadline(deadline: Option<&TransactionDeadline>) -> CoreResult<()> {
    if deadline.is_some_and(TransactionDeadline::is_exceeded) {
        Err(CoreError::TransactionDeadlineExceeded)
    } else {
        Ok(())
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
