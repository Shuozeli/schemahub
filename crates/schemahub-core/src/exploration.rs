//! Schema exploration / read API (design.md §9). Per-declaration storage makes
//! each read a direct object lookup; the core maps the stored blobs through the
//! compiler into summaries / details / type refs.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use schemahub_jj::RefSpec;
use schemahub_types::{
    Action, AuthzError, DeclDetail, DeclSummary, Import, ReadError, ResourcePath, SchemaPath,
    TypeRef,
};

use crate::error::{CoreError, CoreResult};
use crate::request::{
    DeclLocation, DependencyScanSnapshot, DependentsScan, FollowedType, SchemaDependency,
    SchemaDependent, SearchHit,
};
use crate::{Core, SchemaNamePage};

/// A successful reverse-dependency scan never inspects more repositories than
/// this. Crossing the bound fails the entire call so clients never mistake a
/// partial result for a complete visible-repository inventory.
pub const MAX_DEPENDENCY_SCAN_REPOSITORIES: usize = 1_000;

/// Maximum number of schema files inspected by one reverse-dependency scan.
pub const MAX_DEPENDENCY_SCAN_SCHEMAS: usize = 10_000;

/// Bounded dashboard inventory for one schema at an immutable revision.
///
/// Declaration blobs are compiler-validated before they are counted.
/// Dependencies are unique direct imports reported by the format compiler;
/// this summary intentionally does not traverse or authorize import targets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaInventoryStats {
    pub declarations: usize,
    pub dependencies: usize,
}

impl Core {
    /// List schema-file names in a repo at a ref (branch / tag / commit).
    pub fn list_schemas(
        &self,
        project: &str,
        repo: &str,
        at: &RefSpec,
        token: Option<&str>,
    ) -> CoreResult<Vec<String>> {
        let (schemas, _) = self.list_schemas_resolved(project, repo, at, token)?;
        Ok(schemas)
    }

    /// List schemas from one immutable snapshot and return its commit id.
    pub fn list_schemas_resolved(
        &self,
        project: &str,
        repo: &str,
        at: &RefSpec,
        token: Option<&str>,
    ) -> CoreResult<(Vec<String>, String)> {
        let commit_id = self.resolve_read_commit(project, repo, at, token)?;
        let at = RefSpec::commit(commit_id.clone());
        Ok((self.jj.list_schemas(project, repo, &at)?, commit_id))
    }

    /// List one bounded schema-name page from a single immutable snapshot.
    ///
    /// The returned commit must be carried in transport pagination state so a
    /// mutable ref cannot advance between pages and produce a mixed inventory.
    pub fn list_schemas_page_resolved(
        &self,
        project: &str,
        repo: &str,
        at: &RefSpec,
        start_after: Option<&str>,
        limit: usize,
        token: Option<&str>,
    ) -> CoreResult<(SchemaNamePage, String)> {
        if limit == 0 {
            return Err(CoreError::InvalidArgument(
                "schema page limit must be greater than zero".to_string(),
            ));
        }
        if start_after.is_some_and(|cursor| {
            cursor.is_empty()
                || cursor.len() > 1_024
                || cursor.starts_with('/')
                || cursor.ends_with('/')
                || cursor
                    .split('/')
                    .any(|part| part.is_empty() || part == "..")
                || cursor.chars().any(char::is_control)
        }) {
            return Err(CoreError::InvalidArgument(
                "schema page cursor is invalid".to_string(),
            ));
        }
        let commit_id = self.resolve_read_commit(project, repo, at, token)?;
        let at = RefSpec::commit(commit_id.clone());
        Ok((
            self.jj
                .list_schemas_page(project, repo, &at, start_after, limit)?,
            commit_id,
        ))
    }

    /// Summarize a caller-selected schema page with one immutable tree load.
    ///
    /// The ref is resolved once, selected objects and the repository-local
    /// schema inventory are loaded in one JJ traversal, and compiler reads run
    /// only over the selected page. The returned commit identifies the exact
    /// snapshot represented by every summary.
    pub fn summarize_schema_inventory_at(
        &self,
        project: &str,
        repo: &str,
        at: &RefSpec,
        selected_schemas: &BTreeSet<String>,
        token: Option<&str>,
    ) -> CoreResult<(BTreeMap<String, SchemaInventoryStats>, String)> {
        let commit_id = self.resolve_read_commit(project, repo, at, token)?;
        if selected_schemas.is_empty() {
            return Ok((BTreeMap::new(), commit_id));
        }

        let immutable_at = RefSpec::commit(commit_id.clone());
        let batch = self
            .jj
            .load_schemas(project, repo, selected_schemas, &immutable_at)?;
        let same_repo_schemas: HashSet<String> = batch.all_schema_names.into_iter().collect();
        let mut summaries = BTreeMap::new();

        for (schema_name, objects) in batch.schemas {
            let compiler = self.compiler_for(&schema_name)?;
            for declaration in objects.decls.values() {
                compiler.summarize_decl(declaration)?;
            }

            let importing_schema = SchemaPath::new(project, repo, &schema_name);
            let mut direct_dependencies = BTreeSet::new();
            for import in compiler.imports(&objects)? {
                let imported_schema =
                    normalize_import_path(&importing_schema, &import.path, &same_repo_schemas)?;
                direct_dependencies.insert((
                    imported_schema.project,
                    imported_schema.repo,
                    imported_schema.schema_name,
                    import.path,
                    import.resolved_commit,
                    import.decl_name,
                ));
            }

            summaries.insert(
                schema_name,
                SchemaInventoryStats {
                    declarations: objects.decls.len(),
                    dependencies: direct_dependencies.len(),
                },
            );
        }

        Ok((summaries, commit_id))
    }

    /// List declaration summaries in a schema file at a ref (branch / tag /
    /// commit) (DeclBlob → DeclSummary via `compiler.summarize_decl`).
    pub fn list_declarations(
        &self,
        schema: &SchemaPath,
        at: &RefSpec,
        token: Option<&str>,
    ) -> CoreResult<Vec<DeclSummary>> {
        let (declarations, _) = self.list_declarations_resolved(schema, at, token)?;
        Ok(declarations)
    }

    /// List declaration summaries from one immutable snapshot and return its
    /// commit id.
    pub fn list_declarations_resolved(
        &self,
        schema: &SchemaPath,
        at: &RefSpec,
        token: Option<&str>,
    ) -> CoreResult<(Vec<DeclSummary>, String)> {
        let commit_id = self.resolve_read_commit(&schema.project, &schema.repo, at, token)?;
        let at = RefSpec::commit(commit_id.clone());
        let compiler = self.compiler_for(&schema.schema_name)?;
        let names =
            self.jj
                .list_declarations(&schema.project, &schema.repo, &schema.schema_name, &at)?;
        let mut out = Vec::with_capacity(names.len());
        for name in names {
            let blob = self.jj.get_declaration(
                &schema.project,
                &schema.repo,
                &schema.schema_name,
                &name,
                &at,
            )?;
            out.push(compiler.summarize_decl(&blob)?);
        }
        Ok((out, commit_id))
    }

    /// Fetch one declaration's detail at a ref (→ `compiler.decl_detail`).
    pub fn get_declaration(
        &self,
        schema: &SchemaPath,
        at: &RefSpec,
        name: &str,
        token: Option<&str>,
    ) -> CoreResult<DeclDetail> {
        let (_, detail, _) = self.get_declaration_resolved(schema, at, name, token)?;
        Ok(detail)
    }

    /// Fetch one declaration's summary and detail from one immutable,
    /// repository-owned snapshot. The returned commit is exactly the snapshot
    /// used for both renderings.
    pub fn get_declaration_resolved(
        &self,
        schema: &SchemaPath,
        at: &RefSpec,
        name: &str,
        token: Option<&str>,
    ) -> CoreResult<(DeclSummary, DeclDetail, String)> {
        let commit_id = self.resolve_read_commit(&schema.project, &schema.repo, at, token)?;
        let at = RefSpec::commit(commit_id.clone());
        let compiler = self.compiler_for(&schema.schema_name)?;
        let blob = self.jj.get_declaration(
            &schema.project,
            &schema.repo,
            &schema.schema_name,
            name,
            &at,
        )?;
        let summary = compiler.summarize_decl(&blob)?;
        let detail = compiler.decl_detail(&blob)?;
        Ok((summary, detail, commit_id))
    }

    /// Resolve the type names a declaration references (FollowType, design.md §9).
    /// Combines the declaration's `type_refs` with the file's imports so a caller
    /// can locate where each referenced type is defined.
    pub fn follow_type(
        &self,
        schema: &SchemaPath,
        at: &RefSpec,
        name: &str,
        token: Option<&str>,
    ) -> CoreResult<(Vec<TypeRef>, Vec<Import>)> {
        let at =
            RefSpec::commit(self.resolve_read_commit(&schema.project, &schema.repo, at, token)?);
        let compiler = self.compiler_for(&schema.schema_name)?;
        let blob = self.jj.get_declaration(
            &schema.project,
            &schema.repo,
            &schema.schema_name,
            name,
            &at,
        )?;
        let refs = compiler.type_refs(&blob)?;
        let objects =
            self.jj
                .load_schema(&schema.project, &schema.repo, &schema.schema_name, &at)?;
        let imports = compiler.imports(&objects)?;
        Ok((refs, imports))
    }

    /// Follow one named field/property to the exact declaration and immutable
    /// repository snapshot that defines its type.
    pub fn follow_field_type(
        &self,
        schema: &SchemaPath,
        at: &RefSpec,
        declaration_name: &str,
        field_name: &str,
        token: Option<&str>,
    ) -> CoreResult<FollowedType> {
        if declaration_name.is_empty() || field_name.is_empty() {
            return Err(CoreError::InvalidArgument(
                "follow type requires declaration_name and field_name".to_string(),
            ));
        }

        let source_commit = self.resolve_read_commit(&schema.project, &schema.repo, at, token)?;
        let source_at = RefSpec::commit(source_commit.clone());
        let source_compiler = self.compiler_for(&schema.schema_name)?;
        let source_objects = self.jj.load_schema(
            &schema.project,
            &schema.repo,
            &schema.schema_name,
            &source_at,
        )?;
        let source_blob = source_objects
            .decls
            .get(declaration_name)
            .ok_or_else(|| schemahub_jj::JjError::DeclNotFound(declaration_name.to_string()))?;
        let type_ref = source_compiler
            .field_type_ref(source_blob, field_name)?
            .ok_or_else(|| ReadError::NotATypeReference(field_name.to_string()))?;

        if type_ref.import.is_none() {
            if let Some(target_name) = select_declaration_name(
                source_objects.decls.keys().map(String::as_str),
                &type_ref.name,
            )? {
                let target_blob = source_objects
                    .decls
                    .get(&target_name)
                    .expect("selected declaration came from source objects");
                return Ok(FollowedType {
                    source_commit: source_commit.clone(),
                    target_schema: schema.clone(),
                    target_commit: source_commit,
                    summary: source_compiler.summarize_decl(target_blob)?,
                    detail: source_compiler.decl_detail(target_blob)?,
                    pinned: false,
                    import_path: String::new(),
                });
            }
        }

        let imports = match type_ref.import.clone() {
            Some(import) => vec![import],
            None => source_compiler.imports(&source_objects)?,
        };
        let same_repo_schemas: HashSet<String> = self
            .jj
            .list_schemas(&schema.project, &schema.repo, &source_at)?
            .into_iter()
            .collect();
        let mut matches = Vec::new();
        let mut seen = HashSet::new();
        let mut live_snapshots = HashMap::new();

        for import in imports {
            if !import.decl_name.is_empty()
                && !declaration_names_equivalent(&import.decl_name, &type_ref.name)
            {
                continue;
            }
            let target_schema = normalize_import_path(schema, &import.path, &same_repo_schemas)?;
            let target_commit = self.resolve_import_commit(
                schema,
                &source_commit,
                &target_schema,
                &import,
                token,
                &mut live_snapshots,
            )?;
            let target_at = RefSpec::commit(target_commit.clone());
            let target_objects = match self.jj.load_schema(
                &target_schema.project,
                &target_schema.repo,
                &target_schema.schema_name,
                &target_at,
            ) {
                Ok(objects) => objects,
                Err(schemahub_jj::JjError::SchemaNotFound(_)) => continue,
                Err(error) => return Err(error.into()),
            };
            let desired_name = if import.decl_name.is_empty() {
                type_ref.name.as_str()
            } else {
                import.decl_name.as_str()
            };
            let Some(target_name) = select_declaration_name(
                target_objects.decls.keys().map(String::as_str),
                desired_name,
            )?
            else {
                continue;
            };
            if !seen.insert((
                target_schema.clone(),
                target_commit.clone(),
                target_name.clone(),
            )) {
                continue;
            }
            let target_blob = target_objects
                .decls
                .get(&target_name)
                .expect("selected declaration came from target objects");
            let target_compiler = self.compiler_for(&target_schema.schema_name)?;
            matches.push(FollowedType {
                source_commit: source_commit.clone(),
                target_schema,
                target_commit,
                summary: target_compiler.summarize_decl(target_blob)?,
                detail: target_compiler.decl_detail(target_blob)?,
                pinned: !import.resolved_commit.is_empty(),
                import_path: import.path,
            });
        }

        match matches.len() {
            0 => Err(ReadError::NotFound(format!(
                "type '{}' referenced by '{}.{}'",
                type_ref.name, declaration_name, field_name
            ))
            .into()),
            1 => Ok(matches.pop().expect("one match exists")),
            _ => Err(CoreError::FailedPrecondition(format!(
                "type '{}' referenced by '{}.{}' is ambiguous across {} imported schemas",
                type_ref.name,
                declaration_name,
                field_name,
                matches.len()
            ))),
        }
    }

    pub(crate) fn resolve_import_commit(
        &self,
        importing_schema: &SchemaPath,
        importing_commit: &str,
        imported_schema: &SchemaPath,
        import: &Import,
        token: Option<&str>,
        live_snapshots: &mut HashMap<(String, String), String>,
    ) -> CoreResult<String> {
        if !import.resolved_commit.is_empty() {
            return self.resolve_read_commit(
                &imported_schema.project,
                &imported_schema.repo,
                &RefSpec::commit(import.resolved_commit.clone()),
                token,
            );
        }
        if importing_schema.project == imported_schema.project
            && importing_schema.repo == imported_schema.repo
        {
            return Ok(importing_commit.to_string());
        }
        let repository = (
            imported_schema.project.clone(),
            imported_schema.repo.clone(),
        );
        if let Some(commit) = live_snapshots.get(&repository) {
            return Ok(commit.clone());
        }
        let bookmark = self
            .effective_repo_config(&imported_schema.project, &imported_schema.repo)?
            .default_bookmark;
        let commit = self.resolve_read_commit(
            &imported_schema.project,
            &imported_schema.repo,
            &RefSpec::bookmark(bookmark),
            token,
        )?;
        live_snapshots.insert(repository, commit.clone());
        Ok(commit)
    }

    /// List the imports declared in a schema file (design.md §9 ListDependencies).
    pub fn list_dependencies(
        &self,
        schema: &SchemaPath,
        at: &RefSpec,
        transitive: bool,
        token: Option<&str>,
    ) -> CoreResult<Vec<Import>> {
        let (dependencies, _) = self.list_dependencies_detailed(schema, at, transitive, token)?;
        Ok(dependencies
            .into_iter()
            .map(|dependency| dependency.import)
            .collect())
    }

    /// List imports from one immutable traversal root and return that root's
    /// commit id.
    pub fn list_dependencies_resolved(
        &self,
        schema: &SchemaPath,
        at: &RefSpec,
        transitive: bool,
        token: Option<&str>,
    ) -> CoreResult<(Vec<Import>, String)> {
        let (dependencies, commit_id) =
            self.list_dependencies_detailed(schema, at, transitive, token)?;
        Ok((
            dependencies
                .into_iter()
                .map(|dependency| dependency.import)
                .collect(),
            commit_id,
        ))
    }

    /// Return normalized forward edges. Each traversed schema is read from one
    /// immutable snapshot. Unavailable external targets remain explicit edges
    /// with `resolved=false`; storage, corruption, and invalid-pin failures are
    /// never converted into partial success.
    pub fn list_dependencies_detailed(
        &self,
        schema: &SchemaPath,
        at: &RefSpec,
        transitive: bool,
        token: Option<&str>,
    ) -> CoreResult<(Vec<SchemaDependency>, String)> {
        let root_commit = self.resolve_read_commit(&schema.project, &schema.repo, at, token)?;
        let mut queue = VecDeque::from([(schema.clone(), root_commit.clone())]);
        let mut visited = HashSet::new();
        let mut seen_edges = HashSet::new();
        let mut edges = Vec::new();
        let mut live_snapshots = HashMap::new();

        while let Some((importing_schema, importing_commit)) = queue.pop_front() {
            if !visited.insert((importing_schema.clone(), importing_commit.clone())) {
                continue;
            }
            if visited.len() > MAX_DEPENDENCY_SCAN_SCHEMAS {
                return Err(CoreError::ResourceExhausted(format!(
                    "forward-dependency traversal exceeds {MAX_DEPENDENCY_SCAN_SCHEMAS} schema snapshots"
                )));
            }
            let importing_at = RefSpec::commit(importing_commit.clone());
            let compiler = self.compiler_for(&importing_schema.schema_name)?;
            let objects = self.jj.load_schema(
                &importing_schema.project,
                &importing_schema.repo,
                &importing_schema.schema_name,
                &importing_at,
            )?;
            let same_repo_schemas: HashSet<String> = self
                .jj
                .list_schemas(
                    &importing_schema.project,
                    &importing_schema.repo,
                    &importing_at,
                )?
                .into_iter()
                .collect();

            for import in compiler.imports(&objects)? {
                let imported_schema =
                    normalize_import_path(&importing_schema, &import.path, &same_repo_schemas)?;
                let resolution = self.resolve_import_commit(
                    &importing_schema,
                    &importing_commit,
                    &imported_schema,
                    &import,
                    token,
                    &mut live_snapshots,
                );
                let (target_commit, resolved) = match resolution {
                    Ok(commit) => {
                        let target_at = RefSpec::commit(commit.clone());
                        match self.jj.load_schema(
                            &imported_schema.project,
                            &imported_schema.repo,
                            &imported_schema.schema_name,
                            &target_at,
                        ) {
                            Ok(_) => (commit, true),
                            Err(schemahub_jj::JjError::SchemaNotFound(_)) => (commit, false),
                            Err(error) => return Err(error.into()),
                        }
                    }
                    Err(error) if dependency_target_is_unavailable(&error) => {
                        (String::new(), false)
                    }
                    Err(error) => return Err(error),
                };
                let edge_key = (
                    importing_schema.clone(),
                    importing_commit.clone(),
                    imported_schema.clone(),
                    target_commit.clone(),
                    import.path.clone(),
                    import.decl_name.clone(),
                    import.resolved_commit.clone(),
                );
                if seen_edges.insert(edge_key) {
                    edges.push(SchemaDependency {
                        importing_schema: importing_schema.clone(),
                        importing_commit: importing_commit.clone(),
                        imported_schema: imported_schema.clone(),
                        target_commit: target_commit.clone(),
                        resolved,
                        import,
                    });
                }
                if transitive && resolved {
                    queue.push_back((imported_schema, target_commit));
                }
            }
            if !transitive {
                break;
            }
        }

        Ok((edges, root_commit))
    }

    /// Find direct schema imports to `target` across every repository visible
    /// to the caller. Each candidate repository's configured default bookmark
    /// is resolved exactly once, and all reads for that repository use the
    /// returned immutable commit. There is deliberately no cross-repository
    /// global snapshot or automatic propagation; callers persist the returned
    /// snapshot manifest and coordinate downstream ChangeRecords explicitly.
    pub fn list_dependents(
        &self,
        target: &SchemaPath,
        token: Option<&str>,
    ) -> CoreResult<DependentsScan> {
        if target.project.is_empty() || target.repo.is_empty() || target.schema_name.is_empty() {
            return Err(CoreError::InvalidArgument(
                "dependent discovery requires project, repo, and schema_path".to_string(),
            ));
        }
        let identity = self.authn.identify(token)?;
        self.authz.check(
            &identity,
            Action::Read,
            &ResourcePath::repo(&target.project, &target.repo),
        )?;

        let repositories = self.jj.list_repository_keys()?;
        let mut visible_repositories = 0usize;
        let mut schemas_scanned = 0usize;
        let mut snapshots = Vec::new();
        let mut dependents = Vec::new();

        for (project, repo) in repositories {
            match self.authz.check(
                &identity,
                Action::Read,
                &ResourcePath::repo(&project, &repo),
            ) {
                Ok(_) => {}
                Err(AuthzError::PermissionDenied(_)) => continue,
                Err(error) => return Err(error.into()),
            }

            if self
                .repository_store
                .get(&project, &repo)?
                .is_some_and(|repository| repository.archived)
            {
                continue;
            }

            visible_repositories += 1;
            if visible_repositories > MAX_DEPENDENCY_SCAN_REPOSITORIES {
                return Err(CoreError::ResourceExhausted(format!(
                    "reverse-dependency scan exceeds {MAX_DEPENDENCY_SCAN_REPOSITORIES} visible repositories"
                )));
            }

            let bookmark = self
                .effective_repo_config(&project, &repo)?
                .default_bookmark;
            let commit_id =
                match self
                    .jj
                    .resolve_ref_id(&project, &repo, &RefSpec::bookmark(bookmark.clone()))
                {
                    Ok(commit_id) => commit_id,
                    Err(schemahub_jj::JjError::BookmarkNotFound(_)) => continue,
                    Err(error) => return Err(error.into()),
                };
            let at = RefSpec::commit(commit_id.clone());
            let schema_names = self.jj.list_schemas(&project, &repo, &at)?;
            schemas_scanned = schemas_scanned
                .checked_add(schema_names.len())
                .ok_or_else(|| {
                    CoreError::ResourceExhausted(
                        "reverse-dependency schema count overflowed".to_string(),
                    )
                })?;
            if schemas_scanned > MAX_DEPENDENCY_SCAN_SCHEMAS {
                return Err(CoreError::ResourceExhausted(format!(
                    "reverse-dependency scan exceeds {MAX_DEPENDENCY_SCAN_SCHEMAS} schemas"
                )));
            }

            snapshots.push(DependencyScanSnapshot {
                project: project.clone(),
                repo: repo.clone(),
                bookmark: bookmark.clone(),
                commit_id: commit_id.clone(),
            });
            let same_repo_schemas: HashSet<_> = schema_names.iter().cloned().collect();
            for schema_name in schema_names {
                if schema_name == target.schema_name
                    && project == target.project
                    && repo == target.repo
                {
                    continue;
                }
                let compiler = self.compiler_for(&schema_name)?;
                let schema = self.jj.load_schema(&project, &repo, &schema_name, &at)?;
                for import in compiler.imports(&schema)? {
                    let importing_schema = SchemaPath::new(&project, &repo, &schema_name);
                    let imported_schema =
                        normalize_import_path(&importing_schema, &import.path, &same_repo_schemas)?;
                    if imported_schema == *target {
                        dependents.push(SchemaDependent {
                            importing_schema,
                            importing_bookmark: bookmark.clone(),
                            importing_commit: commit_id.clone(),
                            import,
                        });
                    }
                }
            }
        }

        dependents.sort_by(|left, right| {
            left.importing_schema
                .cmp(&right.importing_schema)
                .then_with(|| left.import.decl_name.cmp(&right.import.decl_name))
                .then_with(|| {
                    left.import
                        .resolved_commit
                        .cmp(&right.import.resolved_commit)
                })
        });
        dependents.dedup();
        snapshots.sort_by(|left, right| {
            left.project
                .cmp(&right.project)
                .then_with(|| left.repo.cmp(&right.repo))
        });

        Ok(DependentsScan {
            dependents,
            snapshots,
            schemas_scanned,
        })
    }

    /// Reconstruct canonical source for a schema file at a ref (GetSchemaSource).
    pub fn get_schema_source(
        &self,
        schema: &SchemaPath,
        at: &RefSpec,
        token: Option<&str>,
    ) -> CoreResult<String> {
        let (_, source) = self.get_schema_source_resolved(schema, at, token)?;
        Ok(source)
    }

    /// Reconstruct canonical source and return the exact immutable commit used.
    pub fn get_schema_source_resolved(
        &self,
        schema: &SchemaPath,
        at: &RefSpec,
        token: Option<&str>,
    ) -> CoreResult<(String, String)> {
        let commit_id = self.resolve_read_commit(&schema.project, &schema.repo, at, token)?;
        let at = RefSpec::commit(commit_id.clone());
        let compiler = self.compiler_for(&schema.schema_name)?;
        let objs = self
            .jj
            .load_schema(&schema.project, &schema.repo, &schema.schema_name, &at)?;
        Ok((commit_id, compiler.print(&objs)?))
    }

    /// Search for declarations by name across all schemas in a repo (design.md §9).
    /// `query` matches as a case-insensitive substring of the declaration name.
    pub fn search(
        &self,
        project: &str,
        repo: &str,
        at: &RefSpec,
        query: &str,
        token: Option<&str>,
    ) -> CoreResult<Vec<SearchHit>> {
        let at = RefSpec::commit(self.resolve_read_commit(project, repo, at, token)?);
        let needle = query.to_lowercase();
        let mut hits = Vec::new();
        for schema_name in self.jj.list_schemas(project, repo, &at)? {
            let names = self
                .jj
                .list_declarations(project, repo, &schema_name, &at)?;
            for decl_name in names {
                if decl_name.to_lowercase().contains(&needle) {
                    hits.push(SearchHit {
                        schema_name: schema_name.clone(),
                        decl_name,
                    });
                }
            }
        }
        Ok(hits)
    }

    /// Search returning full summaries (DeclLocation) for richer client display.
    pub fn search_detailed(
        &self,
        project: &str,
        repo: &str,
        at: &RefSpec,
        query: &str,
        token: Option<&str>,
    ) -> CoreResult<Vec<DeclLocation>> {
        let (hits, _) = self.search_detailed_resolved(project, repo, at, query, token)?;
        Ok(hits)
    }

    /// Search one immutable repository snapshot and return its commit id.
    pub fn search_detailed_resolved(
        &self,
        project: &str,
        repo: &str,
        at: &RefSpec,
        query: &str,
        token: Option<&str>,
    ) -> CoreResult<(Vec<DeclLocation>, String)> {
        let commit_id = self.resolve_read_commit(project, repo, at, token)?;
        let at = RefSpec::commit(commit_id.clone());
        let needle = query.to_lowercase();
        let mut hits = Vec::new();
        for schema_name in self.jj.list_schemas(project, repo, &at)? {
            // A successful search is complete for the immutable snapshot. An
            // unknown/corrupt schema must fail the call rather than silently
            // returning a partial result that automation could trust.
            let compiler = self.compiler_for(&schema_name)?;
            for decl_name in self
                .jj
                .list_declarations(project, repo, &schema_name, &at)?
            {
                if !decl_name.to_lowercase().contains(&needle) {
                    continue;
                }
                let blob = self
                    .jj
                    .get_declaration(project, repo, &schema_name, &decl_name, &at)?;
                hits.push(DeclLocation {
                    schema_name: schema_name.clone(),
                    summary: compiler.summarize_decl(&blob)?,
                });
            }
        }
        Ok((hits, commit_id))
    }

    /// Resolve the compiler for a schema file by its extension.
    pub(crate) fn compiler_for(
        &self,
        schema_name: &str,
    ) -> CoreResult<std::sync::Arc<dyn schemahub_types::Compiler>> {
        let format_id = crate::detect_format_from_name(schema_name)
            .ok_or_else(|| CoreError::UndetectableFormat(schema_name.to_string()))?;
        self.registry
            .get(format_id)
            .cloned()
            .ok_or_else(|| CoreError::UnknownFormat(format_id.to_string()))
    }
}

fn parse_logical_import_path(path: &str) -> Option<SchemaPath> {
    let mut parts = path.splitn(3, '/');
    let project = parts.next()?;
    let repo = parts.next()?;
    let schema = parts.next()?;
    if project.is_empty() || repo.is_empty() || schema.is_empty() {
        return None;
    }
    Some(SchemaPath::new(project, repo, schema))
}

pub(crate) fn normalize_import_path(
    importing_schema: &SchemaPath,
    import_path: &str,
    same_repo_schemas: &HashSet<String>,
) -> CoreResult<SchemaPath> {
    if import_path.is_empty() {
        return Err(CoreError::FailedPrecondition(
            "schema contains an empty import path".to_string(),
        ));
    }
    let own_prefix = format!("{}/{}/", importing_schema.project, importing_schema.repo);
    if let Some(schema_name) = import_path.strip_prefix(&own_prefix) {
        if schema_name.is_empty() {
            return Err(CoreError::FailedPrecondition(format!(
                "import path {import_path:?} has no schema component"
            )));
        }
        return Ok(SchemaPath::new(
            &importing_schema.project,
            &importing_schema.repo,
            schema_name,
        ));
    }
    if crate::detect_format_from_name(&importing_schema.schema_name) == Some("openapi")
        && import_path
            .split('/')
            .any(|segment| segment == "." || segment == "..")
    {
        let schema_name =
            normalize_openapi_dot_relative_import(&importing_schema.schema_name, import_path)?;
        return Ok(SchemaPath::new(
            &importing_schema.project,
            &importing_schema.repo,
            schema_name,
        ));
    }
    // Native Protobuf/FlatBuffers paths may contain directories. Prefer an
    // exact schema in the importing snapshot before interpreting the first two
    // segments as SchemaHub's cross-repository project/repo prefix.
    if same_repo_schemas.contains(import_path) {
        return Ok(SchemaPath::new(
            &importing_schema.project,
            &importing_schema.repo,
            import_path,
        ));
    }
    Ok(parse_logical_import_path(import_path).unwrap_or_else(|| {
        SchemaPath::new(
            &importing_schema.project,
            &importing_schema.repo,
            import_path,
        )
    }))
}

fn normalize_openapi_dot_relative_import(
    importing_schema_name: &str,
    import_path: &str,
) -> CoreResult<String> {
    let mut segments: Vec<&str> = importing_schema_name.split('/').collect();
    if segments.is_empty()
        || segments
            .iter()
            .any(|segment| segment.is_empty() || *segment == "." || *segment == "..")
    {
        return Err(CoreError::FailedPrecondition(format!(
            "importing schema path {importing_schema_name:?} cannot anchor a relative import"
        )));
    }
    segments.pop();

    for segment in import_path.split('/') {
        match segment {
            "." => {}
            ".." => {
                if segments.pop().is_none() {
                    return Err(CoreError::FailedPrecondition(format!(
                        "relative import path {import_path:?} escapes the repository root"
                    )));
                }
            }
            "" => {
                return Err(CoreError::FailedPrecondition(format!(
                    "relative import path {import_path:?} contains an empty segment"
                )))
            }
            component => segments.push(component),
        }
    }

    if segments.is_empty() {
        return Err(CoreError::FailedPrecondition(format!(
            "relative import path {import_path:?} has no schema component"
        )));
    }
    Ok(segments.join("/"))
}

fn select_declaration_name<'a>(
    names: impl Iterator<Item = &'a str>,
    requested: &str,
) -> CoreResult<Option<String>> {
    let names: Vec<&str> = names.collect();
    if let Some(exact) = names.iter().find(|name| **name == requested) {
        return Ok(Some((*exact).to_string()));
    }
    let requested = requested.trim_start_matches('.');
    let short = requested.rsplit('.').next().unwrap_or(requested);
    let openapi_schema = format!("schema:{requested}");
    let mut candidates: Vec<&str> = names
        .into_iter()
        .filter(|name| {
            *name == requested
                || *name == short
                || *name == openapi_schema
                || name.rsplit('.').next() == Some(short)
        })
        .collect();
    candidates.sort_unstable();
    candidates.dedup();
    match candidates.as_slice() {
        [] => Ok(None),
        [name] => Ok(Some((*name).to_string())),
        _ => Err(CoreError::FailedPrecondition(format!(
            "type name {requested:?} matches multiple declarations: {}",
            candidates.join(", ")
        ))),
    }
}

fn declaration_names_equivalent(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    let left = left.trim_start_matches('.');
    let right = right.trim_start_matches('.');
    left.rsplit('.').next() == right.rsplit('.').next()
        || left.strip_prefix("schema:") == Some(right)
        || right.strip_prefix("schema:") == Some(left)
}

fn dependency_target_is_unavailable(error: &CoreError) -> bool {
    matches!(
        error,
        CoreError::Authz(AuthzError::PermissionDenied(_))
            | CoreError::Jj(
                schemahub_jj::JjError::BookmarkNotFound(_)
                    | schemahub_jj::JjError::TagNotFound(_)
                    | schemahub_jj::JjError::SchemaNotFound(_)
                    | schemahub_jj::JjError::ObjectNotFound
            )
            | CoreError::Repository(crate::repository::RepositoryError::FailedPrecondition(_))
    )
}
