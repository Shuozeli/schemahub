//! Immutable audit events for mutable project, membership, and repository
//! control-plane resources.
//!
//! Schema/JJ mutations retain their existing repository operation log. This
//! module covers the separate mutable resource-record plane. Production stores
//! append one event in the same `ObjectDb` transaction as the state change it
//! describes, preventing success-without-audit and audit-without-success gaps.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use schemahub_jj::{ObjectDb, ObjectDbError};
use schemahub_types::Role;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::error::CoreResult;
use crate::projects::validate_project_name;
use crate::Core;
use crate::{ProjectMeta, Repository};

pub const AUDIT_COLLECTION_PREFIX: &str = "schemahub.control_plane_audit_events.v1";
pub const AUDIT_INDEX_COLLECTION_PREFIX: &str = "schemahub.control_plane_audit_event_index.v1";

pub fn audit_collection(project: &str) -> String {
    format!(
        "{AUDIT_COLLECTION_PREFIX}/projects/{}",
        hex::encode(project)
    )
}

pub fn audit_index_collection(project: &str) -> String {
    format!(
        "{AUDIT_INDEX_COLLECTION_PREFIX}/projects/{}",
        hex::encode(project)
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlPlaneAuditAction {
    ProjectCreated,
    ProjectUpdated,
    ProjectArchived,
    MemberAdded,
    MemberRoleUpdated,
    MemberRemoved,
    RepositoryCreated,
    RepositoryUpdated,
    RepositoryArchived,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "resource_type", content = "resource", rename_all = "snake_case")]
pub enum ControlPlaneAuditSnapshot {
    Project(ProjectMeta),
    Member {
        identity_id: String,
        role: Role,
        active: bool,
    },
    Repository(Repository),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlPlaneAuditEvent {
    /// `projects/{project}/auditEvents/{event_id}`.
    pub name: String,
    pub event_id: String,
    pub project: String,
    pub resource_name: String,
    pub action: ControlPlaneAuditAction,
    pub actor_id: String,
    pub event_time_unix_ms: i64,
    /// Typed resource snapshot before the mutation. Absent on create.
    pub before: Option<ControlPlaneAuditSnapshot>,
    /// Typed resource snapshot after the mutation. Absent on remove.
    pub after: Option<ControlPlaneAuditSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlPlaneAuditPage {
    pub events: Vec<ControlPlaneAuditEvent>,
    /// Internal ordered-index key. Transport adapters must wrap this in an
    /// opaque, request-bound page token rather than exposing it directly.
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlPlaneAuditContext {
    pub event_id: String,
    pub actor_id: String,
    pub event_time_unix_ms: i64,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ControlPlaneAuditError {
    #[error("control-plane audit clock error: {0}")]
    Clock(String),
    #[error("control-plane audit id error: {0}")]
    Id(String),
    #[error("control-plane audit store error: {0}")]
    Store(String),
}

pub trait ControlPlaneAuditClock: Send + Sync + 'static {
    fn now_unix_millis(&self) -> Result<i64, ControlPlaneAuditError>;
}

pub trait ControlPlaneAuditIdGenerator: Send + Sync + 'static {
    fn new_event_id(&self) -> Result<String, ControlPlaneAuditError>;
}

#[derive(Debug)]
pub struct SystemControlPlaneAuditClock;

impl ControlPlaneAuditClock for SystemControlPlaneAuditClock {
    fn now_unix_millis(&self) -> Result<i64, ControlPlaneAuditError> {
        let duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| ControlPlaneAuditError::Clock(error.to_string()))?;
        i64::try_from(duration.as_millis()).map_err(|_| {
            ControlPlaneAuditError::Clock("system timestamp exceeds i64 milliseconds".to_string())
        })
    }
}

#[derive(Debug)]
pub struct UuidControlPlaneAuditIdGenerator;

impl ControlPlaneAuditIdGenerator for UuidControlPlaneAuditIdGenerator {
    fn new_event_id(&self) -> Result<String, ControlPlaneAuditError> {
        Ok(format!("audit-{}", uuid::Uuid::new_v4().simple()))
    }
}

#[derive(Clone)]
pub struct ControlPlaneAuditRuntime {
    clock: Arc<dyn ControlPlaneAuditClock>,
    ids: Arc<dyn ControlPlaneAuditIdGenerator>,
}

impl ControlPlaneAuditRuntime {
    pub fn new(
        clock: Arc<dyn ControlPlaneAuditClock>,
        ids: Arc<dyn ControlPlaneAuditIdGenerator>,
    ) -> Self {
        Self { clock, ids }
    }

    pub fn production() -> Self {
        Self::new(
            Arc::new(SystemControlPlaneAuditClock),
            Arc::new(UuidControlPlaneAuditIdGenerator),
        )
    }

    pub fn context(
        &self,
        actor_id: impl Into<String>,
    ) -> Result<ControlPlaneAuditContext, ControlPlaneAuditError> {
        Ok(ControlPlaneAuditContext {
            event_id: self.ids.new_event_id()?,
            actor_id: actor_id.into(),
            event_time_unix_ms: self.clock.now_unix_millis()?,
        })
    }
}

impl std::fmt::Debug for ControlPlaneAuditRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ControlPlaneAuditRuntime")
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct ObjectDbControlPlaneAuditLog {
    db: Arc<dyn ObjectDb>,
}

impl ObjectDbControlPlaneAuditLog {
    pub fn new(db: Arc<dyn ObjectDb>) -> Self {
        Self { db }
    }

    /// Read one bounded page in immutable newest-first index order.
    pub fn list_page(
        &self,
        project: &str,
        start_after: Option<&str>,
        limit: usize,
    ) -> Result<ControlPlaneAuditPage, ControlPlaneAuditError> {
        if limit == 0 {
            return Ok(ControlPlaneAuditPage {
                events: Vec::new(),
                next_cursor: None,
            });
        }
        if start_after.is_some_and(|cursor| !is_valid_audit_cursor(cursor)) {
            return Err(ControlPlaneAuditError::Store(
                "control-plane audit cursor is malformed".to_string(),
            ));
        }
        let fetch_limit = limit.checked_add(1).ok_or_else(|| {
            ControlPlaneAuditError::Store("control-plane audit page limit overflow".to_string())
        })?;
        let event_collection = audit_collection(project);
        let index_collection = audit_index_collection(project);
        let mut rows = self
            .db
            .list_records_page(&index_collection, start_after, fetch_limit)
            .map_err(map_object_db)?;
        if rows.is_empty()
            && start_after.is_none()
            && !self
                .db
                .list_records_page(&event_collection, None, 1)
                .map_err(map_object_db)?
                .is_empty()
        {
            return Err(ControlPlaneAuditError::Store(format!(
                "audit index is missing for project {project:?}"
            )));
        }
        let has_more = rows.len() > limit;
        rows.truncate(limit);
        let next_cursor = if has_more {
            rows.last().map(|(key, _)| key.clone())
        } else {
            None
        };
        let mut events = Vec::with_capacity(rows.len());
        for (index_key, event_name_bytes) in rows {
            let event_name = std::str::from_utf8(&event_name_bytes).map_err(|error| {
                ControlPlaneAuditError::Store(format!(
                    "audit index {index_key:?} contains an invalid event name: {error}"
                ))
            })?;
            let bytes = self
                .db
                .get_record(&event_collection, event_name)
                .map_err(map_object_db)?
                .ok_or_else(|| {
                    ControlPlaneAuditError::Store(format!(
                        "audit index {index_key:?} points to missing event {event_name:?}"
                    ))
                })?;
            let event: ControlPlaneAuditEvent =
                serde_json::from_slice(&bytes).map_err(|error| {
                    ControlPlaneAuditError::Store(format!(
                        "decode audit event {event_name:?}: {error}"
                    ))
                })?;
            validate_event(&event, project)?;
            if event.name != event_name {
                return Err(ControlPlaneAuditError::Store(format!(
                    "audit event key/name mismatch: key={event_name:?}, name={:?}",
                    event.name
                )));
            }
            let expected_index_key = audit_index_key(&event)?;
            if index_key != expected_index_key {
                return Err(ControlPlaneAuditError::Store(format!(
                    "audit event index mismatch: key={index_key:?}, expected={expected_index_key:?}"
                )));
            }
            events.push(event);
        }
        Ok(ControlPlaneAuditPage {
            events,
            next_cursor,
        })
    }

    pub fn list(
        &self,
        project: &str,
    ) -> Result<Vec<ControlPlaneAuditEvent>, ControlPlaneAuditError> {
        let mut events = Vec::new();
        let mut cursor = None;
        loop {
            let page = self.list_page(project, cursor.as_deref(), 256)?;
            events.extend(page.events);
            let Some(next_cursor) = page.next_cursor else {
                break;
            };
            cursor = Some(next_cursor);
        }
        Ok(events)
    }
}

impl Core {
    /// List immutable administrative events for one project, newest first.
    ///
    /// Audit snapshots include membership identities and policy state, so this
    /// surface is intentionally Owner-only even when the project is public.
    pub fn list_control_plane_audit_events(
        &self,
        project: &str,
        token: Option<&str>,
    ) -> CoreResult<Vec<ControlPlaneAuditEvent>> {
        self.authorize_control_plane_audit_read(project, token)?;
        Ok(ObjectDbControlPlaneAuditLog::new(self.control_plane_db.clone()).list(project)?)
    }

    /// List one bounded administrative-event page. `start_after` is an
    /// internal immutable-index cursor supplied only after a transport adapter
    /// validates its opaque request-bound page token.
    pub fn list_control_plane_audit_events_page(
        &self,
        project: &str,
        start_after: Option<&str>,
        limit: usize,
        token: Option<&str>,
    ) -> CoreResult<ControlPlaneAuditPage> {
        self.authorize_control_plane_audit_read(project, token)?;
        Ok(
            ObjectDbControlPlaneAuditLog::new(self.control_plane_db.clone()).list_page(
                project,
                start_after,
                limit,
            )?,
        )
    }

    fn authorize_control_plane_audit_read(
        &self,
        project: &str,
        token: Option<&str>,
    ) -> CoreResult<()> {
        validate_project_name(project)?;
        self.project_store
            .get(project)?
            .ok_or_else(|| crate::AccessStoreError::NotFound(project.to_string()))?;
        let identity = self.authn.identify(token)?;
        self.authorize_project_owner(project, &identity)
    }

    pub(crate) fn control_plane_audit_context(
        &self,
        actor_id: impl Into<String>,
    ) -> CoreResult<ControlPlaneAuditContext> {
        Ok(self.control_plane_audit.context(actor_id)?)
    }
}

pub(crate) fn make_event(
    context: &ControlPlaneAuditContext,
    project: &str,
    resource_name: &str,
    action: ControlPlaneAuditAction,
    before: Option<ControlPlaneAuditSnapshot>,
    after: Option<ControlPlaneAuditSnapshot>,
) -> Result<(ControlPlaneAuditEvent, Vec<u8>), ControlPlaneAuditError> {
    let event = ControlPlaneAuditEvent {
        name: format!(
            "projects/{project}/auditEvents/{}",
            context.event_id.as_str()
        ),
        event_id: context.event_id.clone(),
        project: project.to_string(),
        resource_name: resource_name.to_string(),
        action,
        actor_id: context.actor_id.clone(),
        event_time_unix_ms: context.event_time_unix_ms,
        before,
        after,
    };
    validate_event(&event, project)?;
    let bytes = serde_json::to_vec(&event)
        .map_err(|error| ControlPlaneAuditError::Store(error.to_string()))?;
    Ok((event, bytes))
}

pub(crate) fn audit_index_key(
    event: &ControlPlaneAuditEvent,
) -> Result<String, ControlPlaneAuditError> {
    if event.event_time_unix_ms < 0 {
        return Err(ControlPlaneAuditError::Store(
            "audit event time must not precede the Unix epoch".to_string(),
        ));
    }
    if !is_valid_event_id(&event.event_id) {
        return Err(ControlPlaneAuditError::Store(
            "audit event id must be a 1-128 character resource segment".to_string(),
        ));
    }
    let reverse_time = i64::MAX - event.event_time_unix_ms;
    Ok(format!(
        "{reverse_time:019}/{}",
        hex::encode(event.event_id.as_bytes())
    ))
}

/// Validate the internal cursor carried inside an opaque transport page token.
pub fn is_valid_audit_cursor(cursor: &str) -> bool {
    let Some((reverse_time, encoded_event_id)) = cursor.split_once('/') else {
        return false;
    };
    let Ok(reverse_time_value) = reverse_time.parse::<i64>() else {
        return false;
    };
    if reverse_time.len() != 19
        || reverse_time_value < 0
        || format!("{reverse_time_value:019}") != reverse_time
    {
        return false;
    }
    let Ok(event_id_bytes) = hex::decode(encoded_event_id) else {
        return false;
    };
    if hex::encode(&event_id_bytes) != encoded_event_id {
        return false;
    }
    let Ok(event_id) = std::str::from_utf8(&event_id_bytes) else {
        return false;
    };
    is_valid_event_id(event_id)
}

fn is_valid_event_id(event_id: &str) -> bool {
    !event_id.is_empty()
        && event_id.len() <= 128
        && !event_id.contains('/')
        && !event_id.chars().any(char::is_control)
}

fn validate_event(
    event: &ControlPlaneAuditEvent,
    expected_project: &str,
) -> Result<(), ControlPlaneAuditError> {
    if event.project != expected_project {
        return invalid_event(format!(
            "project mismatch: expected {expected_project:?}, found {:?}",
            event.project
        ));
    }
    if !is_valid_event_id(&event.event_id) {
        return invalid_event("event id is not a valid resource segment");
    }
    if event.event_time_unix_ms < 0 {
        return invalid_event("event time precedes the Unix epoch");
    }
    if event.actor_id.trim().is_empty()
        || event.actor_id.len() > 512
        || event.actor_id.chars().any(char::is_control)
    {
        return invalid_event("actor identity is empty, too long, or contains control characters");
    }
    let expected_name = format!("projects/{expected_project}/auditEvents/{}", event.event_id);
    if event.name != expected_name {
        return invalid_event(format!(
            "resource name mismatch: expected {expected_name:?}, found {:?}",
            event.name
        ));
    }

    match event.action {
        ControlPlaneAuditAction::ProjectCreated => {
            require_absent(&event.before, "project-created before")?;
            let after = require_project_snapshot(event.after.as_ref(), "project-created after")?;
            validate_project_snapshot(after, expected_project, &event.resource_name)?;
            if after.archived {
                return invalid_event("a newly created project cannot already be archived");
            }
        }
        ControlPlaneAuditAction::ProjectUpdated => {
            let before = require_project_snapshot(event.before.as_ref(), "project-updated before")?;
            let after = require_project_snapshot(event.after.as_ref(), "project-updated after")?;
            validate_project_snapshot(before, expected_project, &event.resource_name)?;
            validate_project_snapshot(after, expected_project, &event.resource_name)?;
            if before.archived != after.archived {
                return invalid_event("project-updated cannot change archive state");
            }
        }
        ControlPlaneAuditAction::ProjectArchived => {
            let before =
                require_project_snapshot(event.before.as_ref(), "project-archived before")?;
            let after = require_project_snapshot(event.after.as_ref(), "project-archived after")?;
            validate_project_snapshot(before, expected_project, &event.resource_name)?;
            validate_project_snapshot(after, expected_project, &event.resource_name)?;
            if before.archived || !after.archived {
                return invalid_event("project-archived requires an active-to-archived transition");
            }
        }
        ControlPlaneAuditAction::MemberAdded => {
            let (after_id, _, after_active) =
                require_member_snapshot(event.after.as_ref(), "member-added after")?;
            validate_member_snapshot(
                after_id,
                after_active,
                true,
                expected_project,
                &event.resource_name,
            )?;
            if let Some(before) = event.before.as_ref() {
                let (before_id, _, before_active) =
                    require_member_snapshot(Some(before), "member-added before")?;
                validate_member_snapshot(
                    before_id,
                    before_active,
                    false,
                    expected_project,
                    &event.resource_name,
                )?;
                if before_id != after_id {
                    return invalid_event("member-added snapshots identify different members");
                }
            }
        }
        ControlPlaneAuditAction::MemberRoleUpdated => {
            let (before_id, _, before_active) =
                require_member_snapshot(event.before.as_ref(), "member-role-updated before")?;
            let (after_id, _, after_active) =
                require_member_snapshot(event.after.as_ref(), "member-role-updated after")?;
            validate_member_snapshot(
                before_id,
                before_active,
                true,
                expected_project,
                &event.resource_name,
            )?;
            validate_member_snapshot(
                after_id,
                after_active,
                true,
                expected_project,
                &event.resource_name,
            )?;
            if before_id != after_id {
                return invalid_event("member-role-updated snapshots identify different members");
            }
        }
        ControlPlaneAuditAction::MemberRemoved => {
            let (before_id, _, before_active) =
                require_member_snapshot(event.before.as_ref(), "member-removed before")?;
            validate_member_snapshot(
                before_id,
                before_active,
                true,
                expected_project,
                &event.resource_name,
            )?;
            require_absent(&event.after, "member-removed after")?;
        }
        ControlPlaneAuditAction::RepositoryCreated => {
            require_absent(&event.before, "repository-created before")?;
            let after =
                require_repository_snapshot(event.after.as_ref(), "repository-created after")?;
            validate_repository_snapshot(after, expected_project, &event.resource_name)?;
            if after.archived {
                return invalid_event("a newly created repository cannot already be archived");
            }
        }
        ControlPlaneAuditAction::RepositoryUpdated => {
            let before =
                require_repository_snapshot(event.before.as_ref(), "repository-updated before")?;
            let after =
                require_repository_snapshot(event.after.as_ref(), "repository-updated after")?;
            validate_repository_snapshot(before, expected_project, &event.resource_name)?;
            validate_repository_snapshot(after, expected_project, &event.resource_name)?;
            if before.resource_name != after.resource_name || before.archived != after.archived {
                return invalid_event(
                    "repository-updated snapshots change identity or archive state",
                );
            }
        }
        ControlPlaneAuditAction::RepositoryArchived => {
            let before =
                require_repository_snapshot(event.before.as_ref(), "repository-archived before")?;
            let after =
                require_repository_snapshot(event.after.as_ref(), "repository-archived after")?;
            validate_repository_snapshot(before, expected_project, &event.resource_name)?;
            validate_repository_snapshot(after, expected_project, &event.resource_name)?;
            if before.resource_name != after.resource_name || before.archived || !after.archived {
                return invalid_event(
                    "repository-archived requires one active-to-archived resource",
                );
            }
        }
    }
    Ok(())
}

fn require_absent(
    snapshot: &Option<ControlPlaneAuditSnapshot>,
    label: &str,
) -> Result<(), ControlPlaneAuditError> {
    if snapshot.is_some() {
        return invalid_event(format!("{label} snapshot must be absent"));
    }
    Ok(())
}

fn require_project_snapshot<'a>(
    snapshot: Option<&'a ControlPlaneAuditSnapshot>,
    label: &str,
) -> Result<&'a ProjectMeta, ControlPlaneAuditError> {
    match snapshot {
        Some(ControlPlaneAuditSnapshot::Project(project)) => Ok(project),
        _ => invalid_event(format!("{label} snapshot must contain a project")),
    }
}

fn require_member_snapshot<'a>(
    snapshot: Option<&'a ControlPlaneAuditSnapshot>,
    label: &str,
) -> Result<(&'a str, Role, bool), ControlPlaneAuditError> {
    match snapshot {
        Some(ControlPlaneAuditSnapshot::Member {
            identity_id,
            role,
            active,
        }) => Ok((identity_id, *role, *active)),
        _ => invalid_event(format!("{label} snapshot must contain a member")),
    }
}

fn require_repository_snapshot<'a>(
    snapshot: Option<&'a ControlPlaneAuditSnapshot>,
    label: &str,
) -> Result<&'a Repository, ControlPlaneAuditError> {
    match snapshot {
        Some(ControlPlaneAuditSnapshot::Repository(repository)) => Ok(repository),
        _ => invalid_event(format!("{label} snapshot must contain a repository")),
    }
}

fn validate_project_snapshot(
    project: &ProjectMeta,
    expected_project: &str,
    resource_name: &str,
) -> Result<(), ControlPlaneAuditError> {
    let expected_resource = format!("projects/{expected_project}");
    if project.name != expected_project || resource_name != expected_resource {
        return invalid_event("project snapshot does not match the audited resource");
    }
    Ok(())
}

fn validate_member_snapshot(
    identity_id: &str,
    active: bool,
    expected_active: bool,
    expected_project: &str,
    resource_name: &str,
) -> Result<(), ControlPlaneAuditError> {
    let expected_resource = format!(
        "projects/{expected_project}/members/{}",
        hex::encode(identity_id)
    );
    if identity_id.is_empty()
        || identity_id.len() > 512
        || identity_id.chars().any(char::is_control)
        || active != expected_active
        || resource_name != expected_resource
    {
        return invalid_event("member snapshot does not match the audited resource or state");
    }
    Ok(())
}

fn validate_repository_snapshot(
    repository: &Repository,
    expected_project: &str,
    resource_name: &str,
) -> Result<(), ControlPlaneAuditError> {
    let expected_resource = format!("projects/{expected_project}/repos/{}", repository.name);
    if repository.project != expected_project
        || repository.resource_name != expected_resource
        || resource_name != expected_resource
    {
        return invalid_event("repository snapshot does not match the audited resource");
    }
    Ok(())
}

fn invalid_event<T>(reason: impl Into<String>) -> Result<T, ControlPlaneAuditError> {
    Err(ControlPlaneAuditError::Store(format!(
        "invalid control-plane audit event: {}",
        reason.into()
    )))
}

fn map_object_db(error: ObjectDbError) -> ControlPlaneAuditError {
    ControlPlaneAuditError::Store(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use schemahub_jj::MemoryObjectDb;
    use schemahub_types::Visibility;

    use crate::RepoConfig;

    fn context(
        event_id: &str,
        actor_id: &str,
        event_time_unix_ms: i64,
    ) -> ControlPlaneAuditContext {
        ControlPlaneAuditContext {
            event_id: event_id.to_string(),
            actor_id: actor_id.to_string(),
            event_time_unix_ms,
        }
    }

    fn project(name: &str, creator: &str, now_unix_ms: i64) -> ProjectMeta {
        let mut project = ProjectMeta::new(name, Visibility::Private, creator, now_unix_ms);
        project.etag = "v1".to_string();
        project
    }

    fn project_created(
        name: &str,
        event_id: &str,
        actor_id: &str,
        event_time_unix_ms: i64,
    ) -> (ControlPlaneAuditEvent, Vec<u8>) {
        make_event(
            &context(event_id, actor_id, event_time_unix_ms),
            name,
            &format!("projects/{name}"),
            ControlPlaneAuditAction::ProjectCreated,
            None,
            Some(ControlPlaneAuditSnapshot::Project(project(
                name,
                actor_id,
                event_time_unix_ms,
            ))),
        )
        .unwrap()
    }

    fn store_event(db: &dyn ObjectDb, event: &ControlPlaneAuditEvent, bytes: &[u8]) {
        db.create_record(&audit_collection(&event.project), &event.name, bytes)
            .unwrap();
        db.create_record(
            &audit_index_collection(&event.project),
            &audit_index_key(event).unwrap(),
            event.name.as_bytes(),
        )
        .unwrap();
    }

    #[test]
    fn audit_log_is_project_scoped_and_newest_first() {
        // Arrange
        let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
        let log = ObjectDbControlPlaneAuditLog::new(db.clone());
        let (first, first_bytes) = project_created("acme", "audit-a", "alice", 1_000);
        let repository = Repository::new("acme", "api", RepoConfig::default(), "bob", 2_000);
        let repository_name = repository.resource_name.clone();
        let (second, second_bytes) = make_event(
            &context("audit-b", "bob", 2_000),
            "acme",
            &repository_name,
            ControlPlaneAuditAction::RepositoryCreated,
            None,
            Some(ControlPlaneAuditSnapshot::Repository(repository)),
        )
        .unwrap();
        let (other, other_bytes) = project_created("other", "audit-c", "carol", 3_000);
        store_event(db.as_ref(), &first, &first_bytes);
        store_event(db.as_ref(), &second, &second_bytes);
        store_event(db.as_ref(), &other, &other_bytes);

        // Act
        let events = log.list("acme").unwrap();

        // Assert
        assert_eq!(
            events
                .iter()
                .map(|event| event.event_id.as_str())
                .collect::<Vec<_>>(),
            vec!["audit-b", "audit-a"]
        );
    }

    #[test]
    fn audit_log_page_uses_an_exclusive_bounded_cursor() {
        // Arrange
        let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
        let log = ObjectDbControlPlaneAuditLog::new(db.clone());
        for (event_id, event_time) in [("audit-a", 1_000), ("audit-b", 2_000), ("audit-c", 3_000)] {
            let (event, bytes) = project_created("acme", event_id, "alice", event_time);
            store_event(db.as_ref(), &event, &bytes);
        }
        let first = log.list_page("acme", None, 2).unwrap();

        // Act
        let second = log
            .list_page("acme", first.next_cursor.as_deref(), 2)
            .unwrap();

        // Assert
        assert_eq!(
            first
                .events
                .iter()
                .map(|event| event.event_id.as_str())
                .collect::<Vec<_>>(),
            vec!["audit-c", "audit-b"]
        );
        assert!(first.next_cursor.is_some());
        assert_eq!(
            second
                .events
                .iter()
                .map(|event| event.event_id.as_str())
                .collect::<Vec<_>>(),
            vec!["audit-a"]
        );
        assert!(second.next_cursor.is_none());
    }

    #[test]
    fn audit_log_fails_closed_when_an_index_target_is_missing() {
        // Arrange
        let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
        let log = ObjectDbControlPlaneAuditLog::new(db.clone());
        let (event, _) = project_created("acme", "audit-a", "alice", 1_000);
        db.create_record(
            &audit_index_collection("acme"),
            &audit_index_key(&event).unwrap(),
            event.name.as_bytes(),
        )
        .unwrap();

        // Act
        let result = log.list_page("acme", None, 10);

        // Assert
        assert!(matches!(
            result,
            Err(ControlPlaneAuditError::Store(message))
                if message.contains("points to missing event")
        ));
    }

    #[test]
    fn event_builder_rejects_an_action_snapshot_mismatch() {
        // Arrange
        let after = ControlPlaneAuditSnapshot::Member {
            identity_id: "alice".to_string(),
            role: Role::Owner,
            active: true,
        };

        // Act
        let result = make_event(
            &context("audit-a", "alice", 1_000),
            "acme",
            "projects/acme",
            ControlPlaneAuditAction::ProjectCreated,
            None,
            Some(after),
        );

        // Assert
        assert!(matches!(
            result,
            Err(ControlPlaneAuditError::Store(message))
                if message.contains("must contain a project")
        ));
    }

    #[test]
    fn audit_cursor_round_trips_only_canonical_index_keys() {
        // Arrange
        let (event, _) = project_created("acme", "audit-a", "alice", 1_000);
        let cursor = audit_index_key(&event).unwrap();

        // Act
        let valid = is_valid_audit_cursor(&cursor);
        let invalid = is_valid_audit_cursor("not-an-index-key");

        // Assert
        assert!(valid);
        assert!(!invalid);
    }
}
