//! redb/PostgreSQL project and membership stores over `ObjectDb` records.

use std::sync::Arc;

use schemahub_jj::{ObjectDb, ObjectDbError, RecordMutation};
use schemahub_types::{Identity, Role};
use serde::{Deserialize, Serialize};

use crate::auth_store::{
    AccessStoreError, AccessStoreResult, ProjectMeta, ProjectStore, ProjectStorePage, RoleStore,
    RoleStorePage,
};
use crate::control_plane_audit::{
    audit_collection, audit_index_collection, audit_index_key, make_event, ControlPlaneAuditAction,
    ControlPlaneAuditContext, ControlPlaneAuditSnapshot,
};

const PROJECT_COLLECTION: &str = "schemahub.projects.v1";
const ROLE_COLLECTION: &str = "schemahub.project_roles.v1";
const PROJECT_INDEX_PREFIX: &str = "schemahub.project_index.v1";
const PROJECT_INDEX_MIGRATION_COLLECTION: &str = "schemahub.project_index_migration.v1";
const PROJECT_INDEX_MIGRATION_KEY: &str = "complete";
const PROJECT_INDEX_MIGRATION_VALUE: &[u8] = b"schemahub.project_index.v1";
const MAX_CAS_RETRIES: usize = 8;
const DEFAULT_INTERNAL_ROLE_PAGE_SIZE: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct RoleRecord {
    project: String,
    identity_id: String,
    role: Role,
    active: bool,
}

#[derive(Debug)]
pub struct ObjectDbProjectStore {
    db: Arc<dyn ObjectDb>,
}

impl ObjectDbProjectStore {
    pub fn new(db: Arc<dyn ObjectDb>) -> Self {
        Self { db }
    }

    fn encode(meta: &ProjectMeta) -> AccessStoreResult<Vec<u8>> {
        serde_json::to_vec(meta)
            .map_err(|error| AccessStoreError::Backend(format!("encode project: {error}")))
    }

    fn decode(bytes: &[u8]) -> AccessStoreResult<ProjectMeta> {
        serde_json::from_slice(bytes)
            .map_err(|error| AccessStoreError::Backend(format!("decode project: {error}")))
    }

    fn ensure_indexes(&self) -> AccessStoreResult<()> {
        for _ in 0..MAX_CAS_RETRIES {
            match self
                .db
                .get_record(
                    PROJECT_INDEX_MIGRATION_COLLECTION,
                    PROJECT_INDEX_MIGRATION_KEY,
                )
                .map_err(map_db)?
            {
                Some(value) if value == PROJECT_INDEX_MIGRATION_VALUE => return Ok(()),
                Some(_) => {
                    return Err(AccessStoreError::Backend(
                        "project index migration marker is malformed".to_string(),
                    ));
                }
                None => {}
            }

            let mut missing = Vec::new();
            for (key, bytes) in self.db.list_records(PROJECT_COLLECTION).map_err(map_db)? {
                let meta = Self::decode(&bytes)?;
                validate_project_record(&meta, &key)?;
                let index_key = project_index_key(&meta.name)?;
                for collection in project_index_collections(&meta) {
                    match self
                        .db
                        .get_record(&collection, &index_key)
                        .map_err(map_db)?
                    {
                        Some(value) if value == meta.name.as_bytes() => {}
                        Some(_) => {
                            return Err(AccessStoreError::Backend(format!(
                                "project index {collection:?}/{index_key:?} \
                                 does not identify {:?}",
                                meta.name
                            )));
                        }
                        None => missing.push((
                            collection,
                            index_key.clone(),
                            meta.name.as_bytes().to_vec(),
                        )),
                    }
                }
            }
            missing.push((
                PROJECT_INDEX_MIGRATION_COLLECTION.to_string(),
                PROJECT_INDEX_MIGRATION_KEY.to_string(),
                PROJECT_INDEX_MIGRATION_VALUE.to_vec(),
            ));
            let mutations: Vec<_> = missing
                .iter()
                .map(|(collection, key, value)| RecordMutation::Create {
                    collection,
                    key,
                    value,
                })
                .collect();
            if self.db.transact_records(&mutations).map_err(map_db)? {
                return Ok(());
            }
        }
        Err(AccessStoreError::Backend(
            "project index migration did not converge".to_string(),
        ))
    }

    fn upsert(&self, mut meta: ProjectMeta) -> AccessStoreResult<ProjectMeta> {
        self.ensure_indexes()?;
        for _ in 0..MAX_CAS_RETRIES {
            match self
                .db
                .get_record(PROJECT_COLLECTION, &meta.name)
                .map_err(map_db)?
            {
                None => {
                    if meta.etag.is_empty() {
                        meta.etag = "v1".to_string();
                    }
                    let replacement = Self::encode(&meta)?;
                    validate_project_record(&meta, &meta.name)?;
                    let index_key = project_index_key(&meta.name)?;
                    let all_collection = project_index_collection(true);
                    let active_collection = project_index_collection(false);
                    let mut mutations = vec![
                        RecordMutation::Create {
                            collection: PROJECT_COLLECTION,
                            key: &meta.name,
                            value: &replacement,
                        },
                        RecordMutation::Create {
                            collection: &all_collection,
                            key: &index_key,
                            value: meta.name.as_bytes(),
                        },
                    ];
                    if !meta.archived {
                        mutations.push(RecordMutation::Create {
                            collection: &active_collection,
                            key: &index_key,
                            value: meta.name.as_bytes(),
                        });
                    }
                    if self.db.transact_records(&mutations).map_err(map_db)? {
                        return Ok(meta);
                    }
                    if self
                        .db
                        .get_record(PROJECT_COLLECTION, &meta.name)
                        .map_err(map_db)?
                        .is_none()
                    {
                        return Err(AccessStoreError::Backend(format!(
                            "project index collision while creating {:?}",
                            meta.name
                        )));
                    }
                }
                Some(current) => {
                    let current_meta = Self::decode(&current)?;
                    validate_project_record(&current_meta, &meta.name)?;
                    validate_project_replacement(&current_meta, &meta)?;
                    if meta.etag.is_empty() {
                        meta.etag = next_etag(&current_meta.etag)?;
                    }
                    let replacement = Self::encode(&meta)?;
                    let committed = if current_meta.archived == meta.archived {
                        self.db
                            .compare_and_swap_record(
                                PROJECT_COLLECTION,
                                &meta.name,
                                &current,
                                &replacement,
                            )
                            .map_err(map_db)?
                    } else {
                        let index_key = project_index_key(&meta.name)?;
                        let active_collection = project_index_collection(false);
                        let index_mutation = if meta.archived {
                            RecordMutation::CompareAndDelete {
                                collection: &active_collection,
                                key: &index_key,
                                expected: meta.name.as_bytes(),
                            }
                        } else {
                            RecordMutation::Create {
                                collection: &active_collection,
                                key: &index_key,
                                value: meta.name.as_bytes(),
                            }
                        };
                        self.db
                            .transact_records(&[
                                RecordMutation::CompareAndSwap {
                                    collection: PROJECT_COLLECTION,
                                    key: &meta.name,
                                    expected: &current,
                                    replacement: &replacement,
                                },
                                index_mutation,
                            ])
                            .map_err(map_db)?
                    };
                    if committed {
                        return Ok(meta);
                    }
                }
            }
        }
        Err(AccessStoreError::Backend(
            "project changed repeatedly while writing bootstrap metadata".to_string(),
        ))
    }

    /// Atomically import a project and its complete membership set. This is
    /// used by the one-time JSON access-store migration so a crash cannot
    /// leave a project with only a partially copied ACL.
    pub fn create_with_members(
        &self,
        mut meta: ProjectMeta,
        members: &[(String, Role)],
    ) -> AccessStoreResult<ProjectMeta> {
        self.ensure_indexes()?;
        if !members.iter().any(|(_, role)| *role == Role::Owner) {
            return Err(AccessStoreError::Backend(format!(
                "project '{}' cannot be created without an Owner",
                meta.name
            )));
        }
        meta.etag = "v1".to_string();
        validate_project_record(&meta, &meta.name)?;
        let project_bytes = Self::encode(&meta)?;
        let index_key = project_index_key(&meta.name)?;
        let all_collection = project_index_collection(true);
        let active_collection = project_index_collection(false);
        let mut owned_records = vec![
            (
                PROJECT_COLLECTION.to_string(),
                meta.name.clone(),
                project_bytes,
            ),
            (
                all_collection,
                index_key.clone(),
                meta.name.as_bytes().to_vec(),
            ),
        ];
        if !meta.archived {
            owned_records.push((active_collection, index_key, meta.name.as_bytes().to_vec()));
        }
        for (identity_id, role) in members {
            let record = RoleRecord {
                project: meta.name.clone(),
                identity_id: identity_id.clone(),
                role: *role,
                active: true,
            };
            owned_records.push((
                ROLE_COLLECTION.to_string(),
                role_key(&record.project, &record.identity_id),
                encode_role(&record)?,
            ));
        }
        let records: Vec<_> = owned_records
            .iter()
            .map(|(collection, key, value)| (collection.as_str(), key.as_str(), value.as_slice()))
            .collect();
        if !self.db.create_records(&records).map_err(map_db)? {
            return Err(AccessStoreError::AlreadyExists(format!(
                "project '{}' or one of its memberships already exists",
                meta.name
            )));
        }
        Ok(meta)
    }
}

impl ProjectStore for ObjectDbProjectStore {
    fn get(&self, project: &str) -> AccessStoreResult<Option<ProjectMeta>> {
        self.db
            .get_record(PROJECT_COLLECTION, project)
            .map_err(map_db)?
            .map(|bytes| Self::decode(&bytes))
            .transpose()
    }

    fn create_with_owner(
        &self,
        meta: ProjectMeta,
        owner_id: &str,
    ) -> AccessStoreResult<ProjectMeta> {
        self.create_with_members(meta, &[(owner_id.to_string(), Role::Owner)])
    }

    fn create_with_owner_audited(
        &self,
        mut meta: ProjectMeta,
        owner_id: &str,
        audit: &ControlPlaneAuditContext,
    ) -> AccessStoreResult<ProjectMeta> {
        self.ensure_indexes()?;
        meta.etag = "v1".to_string();
        validate_project_record(&meta, &meta.name)?;
        let project_bytes = Self::encode(&meta)?;
        let project_index_key = project_index_key(&meta.name)?;
        let project_all_collection = project_index_collection(true);
        let project_active_collection = project_index_collection(false);
        let owner = RoleRecord {
            project: meta.name.clone(),
            identity_id: owner_id.to_string(),
            role: Role::Owner,
            active: true,
        };
        let owner_key = role_key(&owner.project, &owner.identity_id);
        let owner_bytes = encode_role(&owner)?;
        let resource_name = format!("projects/{}", meta.name);
        let (event, event_bytes) = make_event(
            audit,
            &meta.name,
            &resource_name,
            ControlPlaneAuditAction::ProjectCreated,
            None,
            Some(ControlPlaneAuditSnapshot::Project(meta.clone())),
        )
        .map_err(map_audit)?;
        let audit_collection = audit_collection(&meta.name);
        let audit_index_collection = audit_index_collection(&meta.name);
        let audit_index_key = audit_index_key(&event).map_err(map_audit)?;
        let mut mutations = vec![
            RecordMutation::Create {
                collection: PROJECT_COLLECTION,
                key: &meta.name,
                value: &project_bytes,
            },
            RecordMutation::Create {
                collection: ROLE_COLLECTION,
                key: &owner_key,
                value: &owner_bytes,
            },
            RecordMutation::Create {
                collection: &project_all_collection,
                key: &project_index_key,
                value: meta.name.as_bytes(),
            },
            RecordMutation::Create {
                collection: &audit_collection,
                key: &event.name,
                value: &event_bytes,
            },
            RecordMutation::Create {
                collection: &audit_index_collection,
                key: &audit_index_key,
                value: event.name.as_bytes(),
            },
        ];
        if !meta.archived {
            mutations.push(RecordMutation::Create {
                collection: &project_active_collection,
                key: &project_index_key,
                value: meta.name.as_bytes(),
            });
        }
        if !self.db.transact_records(&mutations).map_err(map_db)? {
            return Err(AccessStoreError::AlreadyExists(format!(
                "project '{}' or its bootstrap membership/audit event already exists",
                meta.name
            )));
        }
        Ok(meta)
    }

    fn set(&self, meta: ProjectMeta) -> AccessStoreResult<()> {
        self.upsert(meta).map(|_| ())
    }

    fn replace(
        &self,
        expected_etag: &str,
        mut meta: ProjectMeta,
    ) -> AccessStoreResult<ProjectMeta> {
        self.ensure_indexes()?;
        let current = self
            .db
            .get_record(PROJECT_COLLECTION, &meta.name)
            .map_err(map_db)?
            .ok_or_else(|| AccessStoreError::NotFound(meta.name.clone()))?;
        let current_meta = Self::decode(&current)?;
        validate_project_record(&current_meta, &meta.name)?;
        if current_meta.etag != expected_etag {
            return Err(AccessStoreError::EtagMismatch {
                name: meta.name,
                expected: expected_etag.to_string(),
                current: current_meta.etag,
            });
        }
        validate_project_replacement(&current_meta, &meta)?;
        meta.etag = next_etag(&current_meta.etag)?;
        let replacement = Self::encode(&meta)?;
        let committed = if current_meta.archived == meta.archived {
            self.db
                .compare_and_swap_record(PROJECT_COLLECTION, &meta.name, &current, &replacement)
                .map_err(map_db)?
        } else {
            let index_key = project_index_key(&meta.name)?;
            let active_collection = project_index_collection(false);
            let index_mutation = if meta.archived {
                RecordMutation::CompareAndDelete {
                    collection: &active_collection,
                    key: &index_key,
                    expected: meta.name.as_bytes(),
                }
            } else {
                RecordMutation::Create {
                    collection: &active_collection,
                    key: &index_key,
                    value: meta.name.as_bytes(),
                }
            };
            self.db
                .transact_records(&[
                    RecordMutation::CompareAndSwap {
                        collection: PROJECT_COLLECTION,
                        key: &meta.name,
                        expected: &current,
                        replacement: &replacement,
                    },
                    index_mutation,
                ])
                .map_err(map_db)?
        };
        if committed {
            return Ok(meta);
        }
        let latest = self
            .get(&meta.name)?
            .ok_or_else(|| AccessStoreError::NotFound(meta.name.clone()))?;
        if latest.etag == expected_etag {
            return Err(AccessStoreError::Backend(format!(
                "project index precondition failed for {:?}",
                meta.name
            )));
        }
        Err(AccessStoreError::EtagMismatch {
            name: meta.name,
            expected: expected_etag.to_string(),
            current: latest.etag,
        })
    }

    fn replace_audited(
        &self,
        expected_etag: &str,
        mut meta: ProjectMeta,
        audit: &ControlPlaneAuditContext,
    ) -> AccessStoreResult<ProjectMeta> {
        self.ensure_indexes()?;
        let current = self
            .db
            .get_record(PROJECT_COLLECTION, &meta.name)
            .map_err(map_db)?
            .ok_or_else(|| AccessStoreError::NotFound(meta.name.clone()))?;
        let current_meta = Self::decode(&current)?;
        validate_project_record(&current_meta, &meta.name)?;
        if current_meta.etag != expected_etag {
            return Err(AccessStoreError::EtagMismatch {
                name: meta.name,
                expected: expected_etag.to_string(),
                current: current_meta.etag,
            });
        }
        validate_project_replacement(&current_meta, &meta)?;
        meta.etag = next_etag(&current_meta.etag)?;
        let replacement = Self::encode(&meta)?;
        let action = if !current_meta.archived && meta.archived {
            ControlPlaneAuditAction::ProjectArchived
        } else {
            ControlPlaneAuditAction::ProjectUpdated
        };
        let resource_name = format!("projects/{}", meta.name);
        let (event, event_bytes) = make_event(
            audit,
            &meta.name,
            &resource_name,
            action,
            Some(ControlPlaneAuditSnapshot::Project(current_meta.clone())),
            Some(ControlPlaneAuditSnapshot::Project(meta.clone())),
        )
        .map_err(map_audit)?;
        let audit_collection = audit_collection(&meta.name);
        let audit_index_collection = audit_index_collection(&meta.name);
        let audit_index_key = audit_index_key(&event).map_err(map_audit)?;
        let active_index_key = (current_meta.archived != meta.archived)
            .then(|| project_index_key(&meta.name))
            .transpose()?;
        let active_collection =
            (current_meta.archived != meta.archived).then(|| project_index_collection(false));
        let mut mutations = vec![
            RecordMutation::CompareAndSwap {
                collection: PROJECT_COLLECTION,
                key: &meta.name,
                expected: &current,
                replacement: &replacement,
            },
            RecordMutation::Create {
                collection: &audit_collection,
                key: &event.name,
                value: &event_bytes,
            },
            RecordMutation::Create {
                collection: &audit_index_collection,
                key: &audit_index_key,
                value: event.name.as_bytes(),
            },
        ];
        if let (Some(index_key), Some(active_collection)) =
            (active_index_key.as_ref(), active_collection.as_ref())
        {
            mutations.push(if meta.archived {
                RecordMutation::CompareAndDelete {
                    collection: active_collection,
                    key: index_key,
                    expected: meta.name.as_bytes(),
                }
            } else {
                RecordMutation::Create {
                    collection: active_collection,
                    key: index_key,
                    value: meta.name.as_bytes(),
                }
            });
        }
        if self.db.transact_records(&mutations).map_err(map_db)? {
            return Ok(meta);
        }
        let latest = self
            .get(&meta.name)?
            .ok_or_else(|| AccessStoreError::NotFound(meta.name.clone()))?;
        if latest.etag == expected_etag {
            return Err(AccessStoreError::Backend(format!(
                "project audit/index precondition failed for event '{}'",
                event.event_id,
            )));
        }
        Err(AccessStoreError::EtagMismatch {
            name: meta.name,
            expected: expected_etag.to_string(),
            current: latest.etag,
        })
    }

    fn list(&self) -> AccessStoreResult<Vec<ProjectMeta>> {
        let mut projects = Vec::new();
        for (key, bytes) in self.db.list_records(PROJECT_COLLECTION).map_err(map_db)? {
            let meta = Self::decode(&bytes)?;
            if meta.name != key {
                return Err(AccessStoreError::Backend(format!(
                    "project key/name mismatch: key={key:?}, name={:?}",
                    meta.name
                )));
            }
            projects.push(meta);
        }
        projects.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(projects)
    }

    fn list_page(
        &self,
        include_archived: bool,
        name_prefix: &str,
        start_after: Option<&str>,
        limit: usize,
    ) -> AccessStoreResult<ProjectStorePage> {
        validate_project_filter(name_prefix, start_after)?;
        if limit == 0 {
            return Ok(ProjectStorePage {
                projects: Vec::new(),
                next_cursor: None,
            });
        }
        self.ensure_indexes()?;
        let collection = project_index_collection(include_archived);
        let start_after = project_page_start(name_prefix, start_after)?;
        let fetch_limit = limit
            .checked_add(1)
            .ok_or_else(|| AccessStoreError::Backend("project page limit overflow".to_string()))?;
        let rows = self
            .db
            .list_records_page(&collection, start_after.as_deref(), fetch_limit)
            .map_err(map_db)?;
        let encoded_prefix = hex::encode(name_prefix.as_bytes());
        let mut projects = Vec::with_capacity(rows.len().min(limit));
        for (index_key, name_bytes) in rows {
            if !index_key.starts_with(&encoded_prefix) {
                break;
            }
            let name = std::str::from_utf8(&name_bytes).map_err(|error| {
                AccessStoreError::Backend(format!(
                    "project index {collection:?}/{index_key:?} contains an invalid name: {error}"
                ))
            })?;
            let bytes = self
                .db
                .get_record(PROJECT_COLLECTION, name)
                .map_err(map_db)?
                .ok_or_else(|| {
                    AccessStoreError::Backend(format!(
                        "project index {collection:?}/{index_key:?} points to missing project {name:?}"
                    ))
                })?;
            let meta = Self::decode(&bytes)?;
            validate_project_record(&meta, name)?;
            if !include_archived && meta.archived {
                return Err(AccessStoreError::Backend(format!(
                    "active project index contains archived project {:?}",
                    meta.name
                )));
            }
            if !meta.name.starts_with(name_prefix) {
                return Err(AccessStoreError::Backend(format!(
                    "project index prefix mismatch for {:?}",
                    meta.name
                )));
            }
            let expected_index_key = project_index_key(&meta.name)?;
            if index_key != expected_index_key {
                return Err(AccessStoreError::Backend(format!(
                    "project index key mismatch: key={index_key:?}, expected={expected_index_key:?}"
                )));
            }
            projects.push(meta);
        }
        let has_more = projects.len() > limit;
        projects.truncate(limit);
        let next_cursor = has_more
            .then(|| projects.last().map(|project| project.name.clone()))
            .flatten();
        Ok(ProjectStorePage {
            projects,
            next_cursor,
        })
    }
}

#[derive(Debug)]
pub struct ObjectDbRoleStore {
    db: Arc<dyn ObjectDb>,
}

impl ObjectDbRoleStore {
    pub fn new(db: Arc<dyn ObjectDb>) -> Self {
        Self { db }
    }

    fn replace(&self, record: RoleRecord) -> AccessStoreResult<()> {
        let key = role_key(&record.project, &record.identity_id);
        let replacement = encode_role(&record)?;
        for _ in 0..MAX_CAS_RETRIES {
            match self.db.get_record(ROLE_COLLECTION, &key).map_err(map_db)? {
                None => {
                    if self
                        .db
                        .create_record(ROLE_COLLECTION, &key, &replacement)
                        .map_err(map_db)?
                    {
                        return Ok(());
                    }
                }
                Some(current) => {
                    if self
                        .db
                        .compare_and_swap_record(ROLE_COLLECTION, &key, &current, &replacement)
                        .map_err(map_db)?
                    {
                        return Ok(());
                    }
                }
            }
        }
        Err(AccessStoreError::Backend(format!(
            "membership {key:?} changed repeatedly"
        )))
    }
}

impl RoleStore for ObjectDbRoleStore {
    fn get(&self, project: &str, identity: &Identity) -> AccessStoreResult<Option<Role>> {
        let Some(identity_id) = identity.id() else {
            return Ok(None);
        };
        self.db
            .get_record(ROLE_COLLECTION, &role_key(project, identity_id))
            .map_err(map_db)?
            .map(|bytes| decode_role(&bytes))
            .transpose()
            .map(|record| {
                record
                    .filter(|record| record.active)
                    .map(|record| record.role)
            })
    }

    fn set(&self, project: &str, identity_id: &str, role: Role) -> AccessStoreResult<()> {
        self.replace(RoleRecord {
            project: project.to_string(),
            identity_id: identity_id.to_string(),
            role,
            active: true,
        })
    }

    fn set_audited(
        &self,
        project: &str,
        identity_id: &str,
        role: Role,
        audit: &ControlPlaneAuditContext,
    ) -> AccessStoreResult<()> {
        let key = role_key(project, identity_id);
        let replacement_record = RoleRecord {
            project: project.to_string(),
            identity_id: identity_id.to_string(),
            role,
            active: true,
        };
        let replacement = encode_role(&replacement_record)?;
        for _ in 0..MAX_CAS_RETRIES {
            let current = self.db.get_record(ROLE_COLLECTION, &key).map_err(map_db)?;
            let current_record = current.as_deref().map(decode_role).transpose()?;
            let action = if current_record.as_ref().is_some_and(|record| record.active) {
                ControlPlaneAuditAction::MemberRoleUpdated
            } else {
                ControlPlaneAuditAction::MemberAdded
            };
            let (event, event_bytes) = make_event(
                audit,
                project,
                &key,
                action,
                current_record.as_ref().map(member_audit_snapshot),
                Some(member_audit_snapshot(&replacement_record)),
            )
            .map_err(map_audit)?;
            let audit_collection = audit_collection(project);
            let audit_index_collection = audit_index_collection(project);
            let audit_index_key = audit_index_key(&event).map_err(map_audit)?;
            let resource_mutation = match current.as_deref() {
                None => RecordMutation::Create {
                    collection: ROLE_COLLECTION,
                    key: &key,
                    value: &replacement,
                },
                Some(expected) => RecordMutation::CompareAndSwap {
                    collection: ROLE_COLLECTION,
                    key: &key,
                    expected,
                    replacement: &replacement,
                },
            };
            let mutations = [
                resource_mutation,
                RecordMutation::Create {
                    collection: &audit_collection,
                    key: &event.name,
                    value: &event_bytes,
                },
                RecordMutation::Create {
                    collection: &audit_index_collection,
                    key: &audit_index_key,
                    value: event.name.as_bytes(),
                },
            ];
            if self.db.transact_records(&mutations).map_err(map_db)? {
                return Ok(());
            }
            if self
                .db
                .get_record(&audit_collection, &event.name)
                .map_err(map_db)?
                .is_some()
            {
                return Err(AccessStoreError::Backend(format!(
                    "control-plane audit event '{}' already exists",
                    event.event_id
                )));
            }
        }
        Err(AccessStoreError::Backend(format!(
            "membership {key:?} changed repeatedly"
        )))
    }

    fn remove(&self, project: &str, identity_id: &str) -> AccessStoreResult<()> {
        let key = role_key(project, identity_id);
        let Some(bytes) = self.db.get_record(ROLE_COLLECTION, &key).map_err(map_db)? else {
            return Ok(());
        };
        let mut record = decode_role(&bytes)?;
        if !record.active {
            return Ok(());
        }
        record.active = false;
        self.replace(record)
    }

    fn remove_audited(
        &self,
        project: &str,
        identity_id: &str,
        audit: &ControlPlaneAuditContext,
    ) -> AccessStoreResult<()> {
        let key = role_key(project, identity_id);
        for _ in 0..MAX_CAS_RETRIES {
            let Some(current) = self.db.get_record(ROLE_COLLECTION, &key).map_err(map_db)? else {
                return Ok(());
            };
            let current_record = decode_role(&current)?;
            if !current_record.active {
                return Ok(());
            }
            let mut replacement_record = current_record.clone();
            replacement_record.active = false;
            let replacement = encode_role(&replacement_record)?;
            let (event, event_bytes) = make_event(
                audit,
                project,
                &key,
                ControlPlaneAuditAction::MemberRemoved,
                Some(member_audit_snapshot(&current_record)),
                None,
            )
            .map_err(map_audit)?;
            let audit_collection = audit_collection(project);
            let audit_index_collection = audit_index_collection(project);
            let audit_index_key = audit_index_key(&event).map_err(map_audit)?;
            let mutations = [
                RecordMutation::CompareAndSwap {
                    collection: ROLE_COLLECTION,
                    key: &key,
                    expected: &current,
                    replacement: &replacement,
                },
                RecordMutation::Create {
                    collection: &audit_collection,
                    key: &event.name,
                    value: &event_bytes,
                },
                RecordMutation::Create {
                    collection: &audit_index_collection,
                    key: &audit_index_key,
                    value: event.name.as_bytes(),
                },
            ];
            if self.db.transact_records(&mutations).map_err(map_db)? {
                return Ok(());
            }
            if self
                .db
                .get_record(&audit_collection, &event.name)
                .map_err(map_db)?
                .is_some()
            {
                return Err(AccessStoreError::Backend(format!(
                    "control-plane audit event '{}' already exists",
                    event.event_id
                )));
            }
        }
        Err(AccessStoreError::Backend(format!(
            "membership {key:?} changed repeatedly"
        )))
    }

    fn list_project(&self, project: &str) -> AccessStoreResult<Vec<(String, Role)>> {
        let mut members = Vec::new();
        let mut cursor = None;
        loop {
            let page = self.list_project_page(
                project,
                cursor.as_deref(),
                DEFAULT_INTERNAL_ROLE_PAGE_SIZE,
            )?;
            members.extend(page.members);
            let Some(next_cursor) = page.next_cursor else {
                break;
            };
            cursor = Some(next_cursor);
        }
        Ok(members)
    }

    fn list_project_page(
        &self,
        project: &str,
        start_after: Option<&str>,
        limit: usize,
    ) -> AccessStoreResult<RoleStorePage> {
        validate_catalog_segment("project", project)?;
        if let Some(cursor) = start_after {
            validate_member_identity("member cursor", cursor)?;
        }
        if limit == 0 {
            return Ok(RoleStorePage {
                members: Vec::new(),
                next_cursor: None,
            });
        }
        let prefix = role_prefix(project);
        if self
            .db
            .get_record(ROLE_COLLECTION, &prefix)
            .map_err(map_db)?
            .is_some()
        {
            return Err(AccessStoreError::Backend(format!(
                "role collection contains an empty member identity for project {project:?}"
            )));
        }
        let start_key = start_after
            .map(|cursor| role_key(project, cursor))
            .unwrap_or_else(|| prefix.clone());
        let fetch_limit = limit
            .checked_add(1)
            .ok_or_else(|| AccessStoreError::Backend("member page limit overflow".to_string()))?;
        let rows = self
            .db
            .list_records_page(ROLE_COLLECTION, Some(&start_key), fetch_limit)
            .map_err(map_db)?;
        let mut members = Vec::with_capacity(rows.len().min(limit));
        let mut last_cursor = None;
        let mut has_more = false;
        for (scanned, (key, bytes)) in rows.into_iter().enumerate() {
            if !key.starts_with(&prefix) {
                break;
            }
            let record = decode_role(&bytes)?;
            validate_role_record(&record, &key)?;
            if scanned == limit {
                has_more = true;
                break;
            }
            last_cursor = Some(record.identity_id.clone());
            if record.active {
                members.push((record.identity_id, record.role));
            }
        }
        Ok(RoleStorePage {
            members,
            next_cursor: has_more.then_some(last_cursor).flatten(),
        })
    }
}

fn role_key(project: &str, identity_id: &str) -> String {
    format!("{}{}", role_prefix(project), hex::encode(identity_id))
}

fn role_prefix(project: &str) -> String {
    format!("projects/{project}/members/")
}

fn validate_member_identity(label: &str, identity_id: &str) -> AccessStoreResult<()> {
    if identity_id.is_empty()
        || identity_id.len() > 512
        || identity_id.chars().any(char::is_control)
    {
        return Err(AccessStoreError::Backend(format!(
            "{label} must be a 1-512 byte identity without control characters"
        )));
    }
    Ok(())
}

fn validate_role_record(record: &RoleRecord, key: &str) -> AccessStoreResult<()> {
    validate_catalog_segment("project", &record.project)?;
    validate_member_identity("member identity", &record.identity_id)?;
    if role_key(&record.project, &record.identity_id) != key {
        return Err(AccessStoreError::Backend(format!(
            "role key/content mismatch for {key:?}"
        )));
    }
    Ok(())
}

fn project_index_collection(include_archived: bool) -> String {
    format!(
        "{PROJECT_INDEX_PREFIX}/{}",
        if include_archived { "all" } else { "active" }
    )
}

fn project_index_collections(meta: &ProjectMeta) -> Vec<String> {
    let mut collections = vec![project_index_collection(true)];
    if !meta.archived {
        collections.push(project_index_collection(false));
    }
    collections
}

fn project_index_key(name: &str) -> AccessStoreResult<String> {
    validate_catalog_segment("project", name)?;
    Ok(format!("{}/", hex::encode(name.as_bytes())))
}

fn project_page_start(
    name_prefix: &str,
    start_after: Option<&str>,
) -> AccessStoreResult<Option<String>> {
    if let Some(cursor) = start_after {
        return project_index_key(cursor).map(Some);
    }
    Ok((!name_prefix.is_empty()).then(|| hex::encode(name_prefix.as_bytes())))
}

fn validate_project_filter(name_prefix: &str, start_after: Option<&str>) -> AccessStoreResult<()> {
    if name_prefix.contains('/')
        || name_prefix.chars().any(char::is_control)
        || name_prefix.len() > 128
    {
        return Err(AccessStoreError::Backend(
            "project name prefix must be at most 128 characters without '/' or control characters"
                .to_string(),
        ));
    }
    if let Some(cursor) = start_after {
        validate_catalog_segment("project cursor", cursor)?;
        if !cursor.starts_with(name_prefix) {
            return Err(AccessStoreError::Backend(
                "project cursor is outside the requested name prefix".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_project_record(meta: &ProjectMeta, key: &str) -> AccessStoreResult<()> {
    if meta.name != key {
        return Err(AccessStoreError::Backend(format!(
            "project key/name mismatch: key={key:?}, name={:?}",
            meta.name
        )));
    }
    project_index_key(&meta.name).map(|_| ())
}

fn validate_project_replacement(
    current: &ProjectMeta,
    replacement: &ProjectMeta,
) -> AccessStoreResult<()> {
    if current.name != replacement.name
        || current.creator != replacement.creator
        || current.create_time_unix_ms != replacement.create_time_unix_ms
    {
        return Err(AccessStoreError::Backend(
            "project replacement modifies immutable coordinates".to_string(),
        ));
    }
    Ok(())
}

fn validate_catalog_segment(label: &str, value: &str) -> AccessStoreResult<()> {
    if value.trim().is_empty()
        || value.contains('/')
        || value.chars().any(char::is_control)
        || value.len() > 128
    {
        return Err(AccessStoreError::Backend(format!(
            "{label} must be a 1-128 character resource path segment without control characters"
        )));
    }
    Ok(())
}

fn encode_role(record: &RoleRecord) -> AccessStoreResult<Vec<u8>> {
    serde_json::to_vec(record)
        .map_err(|error| AccessStoreError::Backend(format!("encode role: {error}")))
}

fn decode_role(bytes: &[u8]) -> AccessStoreResult<RoleRecord> {
    serde_json::from_slice(bytes)
        .map_err(|error| AccessStoreError::Backend(format!("decode role: {error}")))
}

fn member_audit_snapshot(record: &RoleRecord) -> ControlPlaneAuditSnapshot {
    ControlPlaneAuditSnapshot::Member {
        identity_id: record.identity_id.clone(),
        role: record.role,
        active: record.active,
    }
}

fn map_db(error: ObjectDbError) -> AccessStoreError {
    AccessStoreError::Backend(error.to_string())
}

fn map_audit(error: crate::ControlPlaneAuditError) -> AccessStoreError {
    AccessStoreError::Backend(error.to_string())
}

fn next_etag(current: &str) -> AccessStoreResult<String> {
    let version = current
        .strip_prefix('v')
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| {
            AccessStoreError::Backend(format!("invalid stored project etag: {current}"))
        })?;
    Ok(format!("v{}", version + 1))
}

#[cfg(test)]
mod tests {
    use schemahub_jj::{MemoryObjectDb, RedbObjectDb};
    use schemahub_types::Visibility;

    use super::*;
    use crate::ObjectDbControlPlaneAuditLog;

    fn project_named(name: &str) -> ProjectMeta {
        ProjectMeta::new(name, Visibility::Private, "alice", 1_000)
    }

    fn project() -> ProjectMeta {
        project_named("acme")
    }

    fn stored_project() -> ProjectMeta {
        let mut project = project();
        project.etag = "v1".to_string();
        project
    }

    fn audit_context(event_id: &str, actor_id: &str) -> ControlPlaneAuditContext {
        ControlPlaneAuditContext {
            event_id: event_id.to_string(),
            actor_id: actor_id.to_string(),
            event_time_unix_ms: 2_000,
        }
    }

    #[test]
    fn project_and_initial_owner_are_created_atomically() {
        // Arrange
        let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
        let projects = ObjectDbProjectStore::new(db.clone());
        let roles = ObjectDbRoleStore::new(db);

        // Act
        let created = projects
            .create_with_owner(project(), "alice")
            .expect("create project and owner");

        // Assert
        assert_eq!(projects.get("acme").unwrap(), Some(created));
        assert_eq!(
            roles.get("acme", &Identity::user("alice")).unwrap(),
            Some(Role::Owner)
        );
    }

    #[test]
    fn audited_project_creation_commits_project_owner_and_event() {
        // Arrange
        let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
        let projects = ObjectDbProjectStore::new(db.clone());
        let roles = ObjectDbRoleStore::new(db.clone());
        let audit = ObjectDbControlPlaneAuditLog::new(db);
        let context = audit_context("audit-create", "alice");

        // Act
        let created = projects
            .create_with_owner_audited(project(), "alice", &context)
            .expect("create project with audit");

        // Assert
        assert_eq!(projects.get("acme").unwrap(), Some(created.clone()));
        assert_eq!(
            roles.get("acme", &Identity::user("alice")).unwrap(),
            Some(Role::Owner)
        );
        let events = audit.list("acme").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].action, ControlPlaneAuditAction::ProjectCreated);
        assert_eq!(events[0].actor_id, "alice");
        assert_eq!(events[0].before, None);
        assert_eq!(
            events[0].after,
            Some(ControlPlaneAuditSnapshot::Project(created))
        );
    }

    #[test]
    fn audit_id_conflict_rolls_back_project_and_owner() {
        // Arrange
        let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
        db.create_record(
            &audit_collection("acme"),
            "projects/acme/auditEvents/audit-create",
            b"reserved",
        )
        .unwrap();
        let projects = ObjectDbProjectStore::new(db.clone());
        let roles = ObjectDbRoleStore::new(db);
        let context = audit_context("audit-create", "alice");

        // Act
        let result = projects.create_with_owner_audited(project(), "alice", &context);

        // Assert
        assert!(matches!(result, Err(AccessStoreError::AlreadyExists(_))));
        assert_eq!(projects.get("acme").unwrap(), None);
        assert_eq!(roles.get("acme", &Identity::user("alice")).unwrap(), None);
    }

    #[test]
    fn audit_index_conflict_rolls_back_project_owner_and_event() {
        // Arrange
        let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
        let mut expected_project = project();
        expected_project.etag = "v1".to_string();
        let context = audit_context("audit-create", "alice");
        let (event, _) = make_event(
            &context,
            "acme",
            "projects/acme",
            ControlPlaneAuditAction::ProjectCreated,
            None,
            Some(ControlPlaneAuditSnapshot::Project(expected_project)),
        )
        .unwrap();
        db.create_record(
            &audit_index_collection("acme"),
            &audit_index_key(&event).unwrap(),
            b"reserved",
        )
        .unwrap();
        let projects = ObjectDbProjectStore::new(db.clone());
        let roles = ObjectDbRoleStore::new(db.clone());

        // Act
        let result = projects.create_with_owner_audited(project(), "alice", &context);

        // Assert
        assert!(matches!(result, Err(AccessStoreError::AlreadyExists(_))));
        assert_eq!(projects.get("acme").unwrap(), None);
        assert_eq!(roles.get("acme", &Identity::user("alice")).unwrap(), None);
        assert_eq!(
            db.get_record(&audit_collection("acme"), &event.name)
                .unwrap(),
            None
        );
    }

    #[test]
    fn audited_member_add_records_actor_and_typed_snapshot() {
        // Arrange
        let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
        let roles = ObjectDbRoleStore::new(db.clone());
        let audit = ObjectDbControlPlaneAuditLog::new(db);
        let context = audit_context("audit-member", "alice");

        // Act
        roles
            .set_audited("acme", "schema-agent", Role::Writer, &context)
            .expect("add member with audit");

        // Assert
        let events = audit.list("acme").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].action, ControlPlaneAuditAction::MemberAdded);
        assert_eq!(events[0].actor_id, "alice");
        assert_eq!(
            events[0].after,
            Some(ControlPlaneAuditSnapshot::Member {
                identity_id: "schema-agent".to_string(),
                role: Role::Writer,
                active: true,
            })
        );
    }

    #[test]
    fn owner_conflict_leaves_no_partial_project() {
        // Arrange
        let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
        let role = RoleRecord {
            project: "acme".to_string(),
            identity_id: "alice".to_string(),
            role: Role::Reader,
            active: true,
        };
        db.create_record(
            ROLE_COLLECTION,
            &role_key("acme", "alice"),
            &encode_role(&role).unwrap(),
        )
        .unwrap();
        let projects = ObjectDbProjectStore::new(db);

        // Act
        let result = projects.create_with_owner(project(), "alice");

        // Assert
        assert!(matches!(result, Err(AccessStoreError::AlreadyExists(_))));
        assert_eq!(projects.get("acme").unwrap(), None);
    }

    #[test]
    fn owner_conflict_leaves_no_partial_project_in_redb() {
        // Arrange
        let temp = tempfile::tempdir().expect("tempdir");
        let db: Arc<dyn ObjectDb> =
            Arc::new(RedbObjectDb::open(temp.path().join("schemahub.redb")).expect("open redb"));
        let roles = ObjectDbRoleStore::new(db.clone());
        roles
            .set("acme", "alice", Role::Reader)
            .expect("seed conflicting role");
        let projects = ObjectDbProjectStore::new(db);

        // Act
        let result = projects.create_with_owner(project(), "alice");

        // Assert
        assert!(matches!(result, Err(AccessStoreError::AlreadyExists(_))));
        assert_eq!(projects.get("acme").unwrap(), None);
    }

    #[test]
    fn project_and_members_survive_redb_restart() {
        // Arrange
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("schemahub.redb");
        {
            let db: Arc<dyn ObjectDb> =
                Arc::new(RedbObjectDb::open(&path).expect("open redb writer"));
            ObjectDbProjectStore::new(db.clone())
                .create_with_owner(project(), "alice")
                .expect("create project");
            ObjectDbRoleStore::new(db)
                .set("acme", "agent", Role::Writer)
                .expect("add agent");
        }
        let db: Arc<dyn ObjectDb> =
            Arc::new(RedbObjectDb::open(&path).expect("reopen redb reader"));
        let projects = ObjectDbProjectStore::new(db.clone());
        let roles = ObjectDbRoleStore::new(db);

        // Act
        let restored_projects = projects
            .list_page(false, "", None, 1)
            .expect("list project catalog");
        let restored_roles = roles
            .list_project_page("acme", None, 10)
            .expect("list member catalog");

        // Assert
        assert_eq!(restored_projects.projects, vec![stored_project()]);
        assert_eq!(restored_projects.next_cursor, None);
        assert_eq!(
            restored_roles.members,
            vec![
                ("agent".to_string(), Role::Writer),
                ("alice".to_string(), Role::Owner)
            ]
        );
        assert_eq!(restored_roles.next_cursor, None);
    }

    #[test]
    fn member_pages_are_project_scoped_and_advance_past_inactive_tombstones() {
        // Arrange
        let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
        let roles = ObjectDbRoleStore::new(db);
        roles
            .set("acme", "a-removed", Role::Reader)
            .expect("seed removed member");
        roles.remove("acme", "a-removed").expect("remove member");
        roles
            .set("acme", "b-active", Role::Writer)
            .expect("seed active member");
        roles
            .set("other", "a-other", Role::Owner)
            .expect("seed other project");

        // Act
        let first = roles.list_project_page("acme", None, 1).unwrap();
        let second = roles
            .list_project_page("acme", first.next_cursor.as_deref(), 1)
            .unwrap();

        // Assert
        assert!(first.members.is_empty());
        assert_eq!(first.next_cursor.as_deref(), Some("a-removed"));
        assert_eq!(second.members, [("b-active".to_string(), Role::Writer)]);
        assert_eq!(second.next_cursor, None);
    }

    #[test]
    fn member_page_fails_closed_on_a_corrupt_scoped_role_record() {
        // Arrange
        let db = Arc::new(MemoryObjectDb::new());
        let roles = ObjectDbRoleStore::new(db.clone());
        roles.set("acme", "alpha", Role::Reader).unwrap();
        db.create_record(ROLE_COLLECTION, &role_key("acme", "beta"), b"not-json")
            .unwrap();

        // Act
        let result = roles.list_project_page("acme", None, 2);

        // Assert
        assert!(matches!(
            result,
            Err(AccessStoreError::Backend(message)) if message.contains("decode role")
        ));
    }

    #[test]
    fn member_page_ignores_an_unrelated_projects_corrupt_role_record() {
        // Arrange
        let db = Arc::new(MemoryObjectDb::new());
        let roles = ObjectDbRoleStore::new(db.clone());
        roles.set("acme", "alice", Role::Owner).unwrap();
        db.create_record(ROLE_COLLECTION, &role_key("other", "corrupt"), b"not-json")
            .unwrap();

        // Act
        let page = roles.list_project_page("acme", None, 1).unwrap();

        // Assert
        assert_eq!(page.members, [("alice".to_string(), Role::Owner)]);
        assert_eq!(page.next_cursor, None);
    }

    #[test]
    fn project_pages_are_prefix_bounded_and_active_index_follows_archive() {
        // Arrange
        let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
        let projects = ObjectDbProjectStore::new(db);
        for name in ["app", "apple", "beta"] {
            projects
                .create_with_owner(project_named(name), "alice")
                .unwrap();
        }
        let first = projects.list_page(false, "app", None, 1).unwrap();
        let mut archived = first.projects[0].clone();
        let expected_etag = archived.etag.clone();
        archived.archived = true;
        archived.archive_time_unix_ms = Some(2_000);
        archived.update_time_unix_ms = 2_000;

        // Act
        let archived = projects.replace(&expected_etag, archived).unwrap();
        let second = projects
            .list_page(false, "app", first.next_cursor.as_deref(), 1)
            .unwrap();
        let active = projects.list_page(false, "app", None, 2).unwrap();
        let all = projects.list_page(true, "app", None, 2).unwrap();

        // Assert
        assert_eq!(first.projects[0].name, "app");
        assert_eq!(first.next_cursor.as_deref(), Some("app"));
        assert_eq!(second.projects[0].name, "apple");
        assert_eq!(active.projects[0].name, "apple");
        assert_eq!(
            all.projects
                .iter()
                .map(|project| project.name.as_str())
                .collect::<Vec<_>>(),
            ["app", "apple"]
        );
        assert_eq!(all.projects[0], archived);
    }

    #[test]
    fn legacy_projects_are_indexed_before_the_first_page_is_served() {
        // Arrange
        let db = Arc::new(MemoryObjectDb::new());
        let mut legacy = project_named("legacy");
        legacy.etag = "v1".to_string();
        db.create_record(
            PROJECT_COLLECTION,
            &legacy.name,
            &ObjectDbProjectStore::encode(&legacy).unwrap(),
        )
        .unwrap();
        let projects = ObjectDbProjectStore::new(db.clone());

        // Act
        let page = projects.list_page(false, "", None, 1).unwrap();

        // Assert
        assert_eq!(page.projects, [legacy.clone()]);
        assert_eq!(
            db.get_record(
                PROJECT_INDEX_MIGRATION_COLLECTION,
                PROJECT_INDEX_MIGRATION_KEY,
            )
            .unwrap(),
            Some(PROJECT_INDEX_MIGRATION_VALUE.to_vec())
        );
        let index_key = project_index_key(&legacy.name).unwrap();
        for collection in project_index_collections(&legacy) {
            assert_eq!(
                db.get_record(&collection, &index_key).unwrap(),
                Some(legacy.name.as_bytes().to_vec())
            );
        }
    }

    #[test]
    fn missing_project_index_target_fails_closed() {
        // Arrange
        let db = Arc::new(MemoryObjectDb::new());
        let projects = ObjectDbProjectStore::new(db.clone());
        let created = projects
            .create_with_owner(project(), "alice")
            .expect("create project");
        let bytes = ObjectDbProjectStore::encode(&created).unwrap();
        assert!(db
            .compare_and_delete_record(PROJECT_COLLECTION, &created.name, &bytes)
            .unwrap());

        // Act
        let result = projects.list_page(false, "", None, 1);

        // Assert
        assert!(matches!(
            result,
            Err(AccessStoreError::Backend(message)) if message.contains("points to missing project")
        ));
    }

    #[test]
    fn project_index_collision_rolls_back_project_and_owner_creation() {
        // Arrange
        let db = Arc::new(MemoryObjectDb::new());
        let projects = ObjectDbProjectStore::new(db.clone());
        let roles = ObjectDbRoleStore::new(db.clone());
        projects.list_page(false, "", None, 1).unwrap();
        let candidate = project_named("collision");
        let index_key = project_index_key(&candidate.name).unwrap();
        let active_collection = project_index_collection(false);
        db.create_record(&active_collection, &index_key, b"reserved")
            .unwrap();

        // Act
        let result = projects.create_with_owner(candidate.clone(), "alice");

        // Assert
        assert!(matches!(result, Err(AccessStoreError::AlreadyExists(_))));
        assert_eq!(projects.get(&candidate.name).unwrap(), None);
        assert_eq!(
            roles
                .get(&candidate.name, &Identity::user("alice"))
                .unwrap(),
            None
        );
        assert_eq!(
            db.get_record(&project_index_collection(true), &index_key)
                .unwrap(),
            None
        );
    }

    #[test]
    fn indexed_project_page_ignores_an_unrelated_corrupt_primary_record() {
        // Arrange
        let db = Arc::new(MemoryObjectDb::new());
        let projects = ObjectDbProjectStore::new(db.clone());
        let created = projects
            .create_with_owner(project(), "alice")
            .expect("create project");
        db.create_record(PROJECT_COLLECTION, "corrupt", b"not-json")
            .unwrap();

        // Act
        let page = projects.list_page(false, "", None, 1).unwrap();

        // Assert
        assert_eq!(page.projects, [created]);
    }

    #[test]
    fn project_replace_rejects_a_stale_etag_without_losing_the_winner() {
        // Arrange
        let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
        let projects = ObjectDbProjectStore::new(db);
        let created = projects
            .create_with_owner(project(), "alice")
            .expect("create project");
        let mut winner = created.clone();
        winner.visibility = Visibility::Public;
        winner.update_time_unix_ms = 2_000;
        let updated = projects
            .replace(&created.etag, winner)
            .expect("replace project");
        let mut stale = created.clone();
        stale.creator = "mallory".to_string();

        // Act
        let result = projects.replace(&created.etag, stale);

        // Assert
        assert!(matches!(
            result,
            Err(AccessStoreError::EtagMismatch {
                expected,
                current,
                ..
            }) if expected == "v1" && current == "v2"
        ));
        assert_eq!(projects.get("acme").unwrap(), Some(updated));
    }
}
