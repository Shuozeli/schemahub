use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use schemahub_jj::{MemoryObjectDb, ObjectDb, RedbObjectDb};
use schemahub_types::{Identity, IdentityKind, SchemaPath};

use super::{
    ApplyAcquisition, ApplyResult, ChangeClock, ChangeEdit, ChangeIdGenerator, ChangeLedger,
    ChangeLedgerError, ChangeRecordStatus, ChangeRecordStore, ChangeRuntimeError, ChangeUpdate,
    CreateChange, MemoryChangeRecordStore, ObjectDbChangeRecordStore, ValidationResult,
};

#[derive(Debug)]
struct FixedClock(AtomicI64);

impl FixedClock {
    fn new(now: i64) -> Self {
        Self(AtomicI64::new(now))
    }

    fn set(&self, now: i64) {
        self.0.store(now, Ordering::SeqCst);
    }
}

impl ChangeClock for FixedClock {
    fn now_unix_millis(&self) -> Result<i64, ChangeRuntimeError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}

#[derive(Debug, Default)]
struct SequenceIds(AtomicUsize);

impl ChangeIdGenerator for SequenceIds {
    fn generate_change_id(&self) -> String {
        let next = self.0.fetch_add(1, Ordering::SeqCst) + 1;
        format!("change-{next}")
    }
}

fn ledger(clock: Arc<FixedClock>) -> ChangeLedger {
    ledger_with_store(Arc::new(MemoryChangeRecordStore::new()), clock)
}

fn ledger_with_store(store: Arc<dyn ChangeRecordStore>, clock: Arc<FixedClock>) -> ChangeLedger {
    ChangeLedger::new(store, clock, Arc::new(SequenceIds::default()))
}

fn create_input() -> CreateChange {
    CreateChange {
        project: "acme".to_string(),
        repo: "commerce".to_string(),
        change_id: None,
        target_bookmark: "main".to_string(),
        base_revision: None,
        title: "Add order currency".to_string(),
        description: "Persist the currency used for settlement".to_string(),
        external_references: Vec::new(),
        edits: Vec::new(),
    }
}

fn delete_edit() -> ChangeEdit {
    ChangeEdit::DeleteSchema {
        schema: SchemaPath::new("acme", "commerce", "legacy.proto"),
        format_id: "protobuf".to_string(),
    }
}

fn ready_change(ledger: &ChangeLedger) -> super::ChangeRecord {
    let mut input = create_input();
    input.edits.push(delete_edit());
    let draft = ledger
        .create(input, &Identity::user("alice"))
        .expect("seed executable draft");
    let validated = ledger
        .record_validation(
            &draft.name,
            &draft.etag,
            ValidationResult {
                valid: true,
                resolved_base_commit: "abc123".to_string(),
                edit_digest: "sha256:edits".to_string(),
                issues: Vec::new(),
                validated_at_unix_ms: 0,
                validator_version: "schemahub-test".to_string(),
            },
        )
        .expect("record validation");
    ledger
        .mark_ready(&validated.name, &validated.etag)
        .expect("mark ready")
}

#[test]
fn create_note_records_server_derived_human_actor_and_time() {
    // Arrange
    let clock = Arc::new(FixedClock::new(1_000));
    let ledger = ledger(clock);
    let actor = Identity::user_with_display("alice", "Alice");

    // Act
    let record = ledger
        .create(create_input(), &actor)
        .expect("create change record");

    // Assert
    assert_eq!(record.name, "projects/acme/repos/commerce/changes/change-1");
    assert_eq!(record.status, ChangeRecordStatus::Draft);
    assert!(record.edits.is_empty());
    assert_eq!(record.created_by.identity, "alice");
    assert_eq!(record.created_by.kind, IdentityKind::Human);
    assert_eq!(record.created_by.display_name.as_deref(), Some("Alice"));
    assert_eq!(record.create_time_unix_ms, 1_000);
    assert_eq!(record.update_time_unix_ms, 1_000);
    assert_eq!(record.etag, "v1");
}

#[test]
fn create_agent_note_preserves_delegation_metadata() {
    // Arrange
    let clock = Arc::new(FixedClock::new(1_000));
    let ledger = ledger(clock);
    let actor = Identity::agent(
        "schema-agent",
        Some("Schema Agent".to_string()),
        Some("alice".to_string()),
    );

    // Act
    let record = ledger
        .create(create_input(), &actor)
        .expect("create agent change record");

    // Assert
    assert_eq!(record.created_by.identity, "schema-agent");
    assert_eq!(record.created_by.kind, IdentityKind::Agent);
    assert_eq!(record.created_by.delegated_by.as_deref(), Some("alice"));
}

#[test]
fn create_note_normalizes_and_preserves_external_references() {
    // Arrange
    let clock = Arc::new(FixedClock::new(1_000));
    let ledger = ledger(clock);
    let mut input = create_input();
    input.external_references = vec![
        "  INC-2048  ".to_string(),
        "https://tracker.example.test/issues/2048".to_string(),
    ];

    // Act
    let record = ledger
        .create(input, &Identity::user("alice"))
        .expect("create referenced change record");

    // Assert
    assert_eq!(
        record.external_references,
        ["INC-2048", "https://tracker.example.test/issues/2048"]
    );
}

#[test]
fn create_note_rejects_duplicate_external_references_after_normalization() {
    // Arrange
    let clock = Arc::new(FixedClock::new(1_000));
    let ledger = ledger(clock);
    let mut input = create_input();
    input.external_references = vec!["INC-2048".to_string(), " INC-2048 ".to_string()];

    // Act
    let result = ledger.create(input, &Identity::user("alice"));

    // Assert
    assert!(matches!(result, Err(ChangeLedgerError::InvalidArgument(_))));
}

#[test]
fn create_note_rejects_more_than_thirty_two_external_references() {
    // Arrange
    let clock = Arc::new(FixedClock::new(1_000));
    let ledger = ledger(clock);
    let mut input = create_input();
    input.external_references = (0..33).map(|index| format!("REF-{index}")).collect();

    // Act
    let result = ledger.create(input, &Identity::user("alice"));

    // Assert
    assert!(result
        .expect_err("too many references must fail")
        .to_string()
        .contains("at most 32"));
}

#[test]
fn create_note_rejects_control_characters_in_external_references() {
    // Arrange
    let clock = Arc::new(FixedClock::new(1_000));
    let ledger = ledger(clock);
    let mut input = create_input();
    input.external_references = vec!["INC-2048\nforged".to_string()];

    // Act
    let result = ledger.create(input, &Identity::user("alice"));

    // Assert
    assert!(result
        .expect_err("control characters must fail")
        .to_string()
        .contains("without control characters"));
}

#[test]
fn stored_records_without_external_references_decode_as_an_empty_list() {
    // Arrange
    let clock = Arc::new(FixedClock::new(1_000));
    let record = ledger(clock)
        .create(create_input(), &Identity::user("alice"))
        .expect("create current record");
    let mut encoded = serde_json::to_value(record).expect("encode record");
    encoded
        .as_object_mut()
        .expect("record object")
        .remove("external_references");

    // Act
    let decoded: super::ChangeRecord = serde_json::from_value(encoded).expect("decode old record");

    // Assert
    assert!(decoded.external_references.is_empty());
}

#[test]
fn create_rejects_control_characters_in_resource_scope() {
    // Arrange
    let clock = Arc::new(FixedClock::new(1_000));
    let ledger = ledger(clock);
    let mut input = create_input();
    input.project = "acme\nforged".to_string();

    // Act
    let result = ledger.create(input, &Identity::user("alice"));

    // Assert
    assert!(matches!(result, Err(ChangeLedgerError::InvalidArgument(_))));
}

#[test]
fn update_draft_uses_etag_and_advances_update_time() {
    // Arrange
    let clock = Arc::new(FixedClock::new(1_000));
    let ledger = ledger(clock.clone());
    let record = ledger
        .create(create_input(), &Identity::user("alice"))
        .expect("seed draft");
    clock.set(2_000);

    // Act
    let updated = ledger
        .update(
            &record.name,
            &record.etag,
            ChangeUpdate {
                description: Some("Updated rationale".to_string()),
                external_references: Some(vec!["DESIGN-17".to_string()]),
                ..ChangeUpdate::default()
            },
        )
        .expect("update draft");

    // Assert
    assert_eq!(updated.description, "Updated rationale");
    assert_eq!(updated.external_references, ["DESIGN-17"]);
    assert_eq!(updated.etag, "v2");
    assert_eq!(updated.create_time_unix_ms, 1_000);
    assert_eq!(updated.update_time_unix_ms, 2_000);
}

#[test]
fn stale_etag_is_rejected_without_overwriting_the_record() {
    // Arrange
    let clock = Arc::new(FixedClock::new(1_000));
    let ledger = ledger(clock);
    let record = ledger
        .create(create_input(), &Identity::user("alice"))
        .expect("seed draft");
    ledger
        .update(
            &record.name,
            &record.etag,
            ChangeUpdate {
                description: Some("first update".to_string()),
                ..ChangeUpdate::default()
            },
        )
        .expect("advance etag");

    // Act
    let result = ledger.update(
        &record.name,
        &record.etag,
        ChangeUpdate {
            description: Some("stale update".to_string()),
            ..ChangeUpdate::default()
        },
    );

    // Assert
    assert!(matches!(
        result,
        Err(ChangeLedgerError::Store(
            super::ChangeStoreError::EtagMismatch { .. }
        ))
    ));
    let current = ledger.get(&record.name).expect("read current record");
    assert_eq!(current.description, "first update");
}

#[test]
fn note_only_draft_cannot_be_marked_ready() {
    // Arrange
    let clock = Arc::new(FixedClock::new(1_000));
    let ledger = ledger(clock);
    let record = ledger
        .create(create_input(), &Identity::user("alice"))
        .expect("seed note-only draft");

    // Act
    let result = ledger.mark_ready(&record.name, &record.etag);

    // Assert
    assert!(matches!(
        result,
        Err(ChangeLedgerError::FailedPrecondition(message))
            if message.contains("note-only")
    ));
}

#[test]
fn validated_executable_draft_can_be_marked_ready() {
    // Arrange
    let clock = Arc::new(FixedClock::new(1_000));
    let ledger = ledger(clock);
    let mut input = create_input();
    input.edits.push(delete_edit());
    let draft = ledger
        .create(input, &Identity::user("alice"))
        .expect("seed executable draft");
    let validated = ledger
        .record_validation(
            &draft.name,
            &draft.etag,
            ValidationResult {
                valid: true,
                resolved_base_commit: "abc123".to_string(),
                edit_digest: "sha256:edits".to_string(),
                issues: Vec::new(),
                validated_at_unix_ms: 0,
                validator_version: "schemahub-test".to_string(),
            },
        )
        .expect("record validation");

    // Act
    let ready = ledger
        .mark_ready(&validated.name, &validated.etag)
        .expect("mark ready");

    // Assert
    assert_eq!(ready.status, ChangeRecordStatus::Ready);
    assert_eq!(ready.etag, "v3");
}

#[test]
fn author_cannot_self_approve_but_another_reviewer_can() {
    // Arrange
    let clock = Arc::new(FixedClock::new(1_000));
    let ledger = ledger(clock);
    let mut input = create_input();
    input.edits.push(delete_edit());
    let draft = ledger
        .create(input, &Identity::user("alice"))
        .expect("seed executable draft");
    let validated = ledger
        .record_validation(
            &draft.name,
            &draft.etag,
            ValidationResult {
                valid: true,
                resolved_base_commit: "abc123".to_string(),
                edit_digest: "sha256:edits".to_string(),
                issues: Vec::new(),
                validated_at_unix_ms: 0,
                validator_version: "schemahub-test".to_string(),
            },
        )
        .expect("record validation");
    let ready = ledger
        .mark_ready(&validated.name, &validated.etag)
        .expect("mark ready");

    // Act
    let self_review = ledger.approve(
        &ready.name,
        &ready.etag,
        &Identity::user("alice"),
        String::new(),
    );
    let approved = ledger
        .approve(
            &ready.name,
            &ready.etag,
            &Identity::user("bob"),
            "Looks compatible".to_string(),
        )
        .expect("independent approval");

    // Assert
    assert!(matches!(
        self_review,
        Err(ChangeLedgerError::FailedPrecondition(message)) if message.contains("own change")
    ));
    assert_eq!(approved.status, ChangeRecordStatus::Ready);
    assert_eq!(approved.reviews.len(), 1);
    assert_eq!(approved.reviews[0].reviewer.identity, "bob");
}

#[test]
fn apply_lease_is_idempotent_and_completion_retains_receipt() {
    // Arrange
    let clock = Arc::new(FixedClock::new(1_000));
    let ledger = ledger(clock);
    let ready = ready_change(&ledger);

    // Act
    let acquired = ledger
        .acquire_apply(
            &ready.name,
            &ready.etag,
            "request-1",
            &Identity::user("alice"),
            10_000,
        )
        .expect("acquire apply lease");
    let applying = match acquired {
        ApplyAcquisition::Acquired(record) => record,
        other => panic!("expected acquired lease, got {other:?}"),
    };
    let observed = ledger
        .acquire_apply(
            &ready.name,
            &ready.etag,
            "request-1",
            &Identity::user("alice"),
            10_000,
        )
        .expect("observe same request");
    let attempt = applying.apply_attempt.as_ref().expect("apply attempt");
    let applied = ledger
        .complete_apply(
            &applying.name,
            "request-1",
            &attempt.attempt_id,
            ApplyResult {
                commit_id: "commit-1".to_string(),
                change_id: "change-1".to_string(),
                operation_id: "operation-1".to_string(),
                conflicted_declarations: Vec::new(),
                artifact_digest: None,
            },
        )
        .expect("complete apply");
    let retried = ledger
        .acquire_apply(
            &ready.name,
            &ready.etag,
            "request-1",
            &Identity::user("alice"),
            10_000,
        )
        .expect("retry completed request");

    // Assert
    assert!(matches!(observed, ApplyAcquisition::Observed(_)));
    assert_eq!(applied.status, ChangeRecordStatus::Applied);
    assert_eq!(
        applied
            .apply_result
            .as_ref()
            .map(|result| result.operation_id.as_str()),
        Some("operation-1")
    );
    assert!(matches!(retried, ApplyAcquisition::AlreadyApplied(_)));
}

#[test]
fn policy_rejection_release_returns_change_to_ready() {
    // Arrange
    let clock = Arc::new(FixedClock::new(1_000));
    let ledger = ledger(clock);
    let ready = ready_change(&ledger);
    let applying = match ledger
        .acquire_apply(
            &ready.name,
            &ready.etag,
            "request-1",
            &Identity::user("alice"),
            10_000,
        )
        .expect("acquire apply lease")
    {
        ApplyAcquisition::Acquired(record) => record,
        other => panic!("expected acquired lease, got {other:?}"),
    };
    let attempt = applying.apply_attempt.as_ref().expect("apply attempt");

    // Act
    let released = ledger
        .release_apply(
            &applying.name,
            &attempt.request_id,
            &attempt.attempt_id,
            &attempt.lease_owner,
        )
        .expect("release rejected publication");
    let reacquired = ledger
        .acquire_apply(
            &released.name,
            &released.etag,
            "request-1",
            &Identity::user("alice"),
            10_000,
        )
        .expect("retry after release");

    // Assert
    assert_eq!(released.status, ChangeRecordStatus::Ready);
    assert!(released.apply_attempt.is_none());
    let ApplyAcquisition::Acquired(retried) = reacquired else {
        panic!("retry must acquire a fresh lease");
    };
    assert_ne!(
        retried.apply_attempt.expect("retry attempt").attempt_id,
        attempt.attempt_id
    );
}

#[test]
fn expired_apply_lease_can_be_reacquired_without_changing_attempt_id() {
    // Arrange
    let clock = Arc::new(FixedClock::new(1_000));
    let ledger = ledger(clock.clone());
    let ready = ready_change(&ledger);
    let applying = match ledger
        .acquire_apply(
            &ready.name,
            &ready.etag,
            "request-1",
            &Identity::user("alice"),
            100,
        )
        .expect("acquire initial lease")
    {
        ApplyAcquisition::Acquired(record) => record,
        other => panic!("expected acquired lease, got {other:?}"),
    };
    let first_attempt = applying.apply_attempt.expect("first attempt");
    clock.set(1_101);

    // Act
    let reacquired = ledger
        .acquire_apply(
            &ready.name,
            &ready.etag,
            "request-1",
            &Identity::user("alice"),
            100,
        )
        .expect("reacquire expired lease");

    // Assert
    let record = match reacquired {
        ApplyAcquisition::Acquired(record) => record,
        other => panic!("expected reacquired lease, got {other:?}"),
    };
    let second_attempt = record.apply_attempt.expect("second attempt");
    assert_eq!(second_attempt.attempt_id, first_attempt.attempt_id);
    assert_ne!(second_attempt.lease_owner, first_attempt.lease_owner);
    assert_eq!(record.etag, "v5");
}

#[test]
fn concurrent_ledger_instances_grant_exactly_one_apply_lease() {
    // Arrange
    const WRITERS: usize = 32;
    let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
    let seed = ledger_with_store(
        Arc::new(ObjectDbChangeRecordStore::new(db.clone())),
        Arc::new(FixedClock::new(1_000)),
    );
    let ready = ready_change(&seed);
    let barrier = Arc::new(Barrier::new(WRITERS + 1));
    let handles: Vec<_> = (0..WRITERS)
        .map(|_| {
            let db = db.clone();
            let ready = ready.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                let ledger = ledger_with_store(
                    Arc::new(ObjectDbChangeRecordStore::new(db)),
                    Arc::new(FixedClock::new(2_000)),
                );
                barrier.wait();
                ledger.acquire_apply(
                    &ready.name,
                    &ready.etag,
                    "shared-request",
                    &Identity::user("agent"),
                    10_000,
                )
            })
        })
        .collect();

    // Act
    barrier.wait();
    let acquisitions: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().expect("writer thread must not panic"))
        .collect::<Result<_, _>>()
        .expect("every writer must acquire or observe the shared attempt");

    // Assert
    assert_eq!(
        acquisitions
            .iter()
            .filter(|result| matches!(result, ApplyAcquisition::Acquired(_)))
            .count(),
        1
    );
    assert_eq!(
        acquisitions
            .iter()
            .filter(|result| matches!(result, ApplyAcquisition::Observed(_)))
            .count(),
        WRITERS - 1
    );
    let persisted = seed.get(&ready.name).expect("read persisted lease");
    assert_eq!(persisted.status, ChangeRecordStatus::Applying);
    assert_eq!(
        persisted
            .apply_attempt
            .as_ref()
            .map(|attempt| attempt.request_id.as_str()),
        Some("shared-request")
    );
}

#[test]
fn concurrent_reconcilers_converge_on_the_first_apply_receipt() {
    // Arrange
    const WRITERS: usize = 32;
    let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
    let seed = ledger_with_store(
        Arc::new(ObjectDbChangeRecordStore::new(db.clone())),
        Arc::new(FixedClock::new(1_000)),
    );
    let ready = ready_change(&seed);
    let applying = match seed
        .acquire_apply(
            &ready.name,
            &ready.etag,
            "shared-request",
            &Identity::user("agent"),
            10_000,
        )
        .expect("seed apply attempt")
    {
        ApplyAcquisition::Acquired(record) => record,
        other => panic!("expected acquired lease, got {other:?}"),
    };
    let attempt_id = applying
        .apply_attempt
        .as_ref()
        .expect("persisted apply attempt")
        .attempt_id
        .clone();
    let barrier = Arc::new(Barrier::new(WRITERS + 1));
    let handles: Vec<_> = (0..WRITERS)
        .map(|writer| {
            let db = db.clone();
            let name = ready.name.clone();
            let attempt_id = attempt_id.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                let ledger = ledger_with_store(
                    Arc::new(ObjectDbChangeRecordStore::new(db)),
                    Arc::new(FixedClock::new(2_000)),
                );
                barrier.wait();
                ledger.complete_apply(
                    &name,
                    "shared-request",
                    &attempt_id,
                    ApplyResult {
                        commit_id: format!("commit-{writer}"),
                        change_id: format!("change-{writer}"),
                        operation_id: format!("operation-{writer}"),
                        conflicted_declarations: Vec::new(),
                        artifact_digest: None,
                    },
                )
            })
        })
        .collect();

    // Act
    barrier.wait();
    let completions: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().expect("reconciler thread must not panic"))
        .collect::<Result<_, _>>()
        .expect("every reconciler must return the persisted winner");

    // Assert
    let persisted = seed.get(&ready.name).expect("read applied record");
    assert_eq!(persisted.status, ChangeRecordStatus::Applied);
    assert_eq!(persisted.etag, "v5");
    assert!(completions
        .iter()
        .all(|record| record.apply_result == persisted.apply_result));
    assert!(persisted
        .apply_result
        .as_ref()
        .is_some_and(|result| result.operation_id.starts_with("operation-")));
}

#[test]
fn abandoned_record_is_immutable() {
    // Arrange
    let clock = Arc::new(FixedClock::new(1_000));
    let ledger = ledger(clock);
    let draft = ledger
        .create(create_input(), &Identity::user("alice"))
        .expect("seed draft");
    let abandoned = ledger
        .abandon(&draft.name, &draft.etag)
        .expect("abandon draft");

    // Act
    let result = ledger.update(
        &abandoned.name,
        &abandoned.etag,
        ChangeUpdate {
            description: Some("must not apply".to_string()),
            ..ChangeUpdate::default()
        },
    );

    // Assert
    assert!(matches!(
        result,
        Err(ChangeLedgerError::FailedPrecondition(_))
    ));
    assert!(abandoned.status.is_terminal());
}

#[test]
fn list_is_scoped_to_one_project_and_repo() {
    // Arrange
    let clock = Arc::new(FixedClock::new(1_000));
    let ledger = ledger(clock);
    ledger
        .create(create_input(), &Identity::user("alice"))
        .expect("seed matching record");
    let mut other = create_input();
    other.repo = "billing".to_string();
    ledger
        .create(other, &Identity::user("alice"))
        .expect("seed other record");

    // Act
    let records = ledger.list("acme", "commerce").expect("list records");

    // Assert
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].repo, "commerce");
}

#[test]
fn object_db_store_is_shared_across_ledger_instances() {
    // Arrange
    let db = Arc::new(MemoryObjectDb::new());
    let writer = ledger_with_store(
        Arc::new(ObjectDbChangeRecordStore::new(db.clone())),
        Arc::new(FixedClock::new(1_000)),
    );
    let created = writer
        .create(create_input(), &Identity::user("alice"))
        .expect("create durable record");
    let reader = ledger_with_store(
        Arc::new(ObjectDbChangeRecordStore::new(db)),
        Arc::new(FixedClock::new(2_000)),
    );

    // Act
    let restored = reader.get(&created.name).expect("restore record");

    // Assert
    assert_eq!(restored, created);
}

#[test]
fn redb_change_record_survives_database_reopen() {
    // Arrange
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("schemahub.redb");
    let name = {
        let db = Arc::new(RedbObjectDb::open(&path).expect("open redb writer"));
        let writer = ledger_with_store(
            Arc::new(ObjectDbChangeRecordStore::new(db)),
            Arc::new(FixedClock::new(1_000)),
        );
        writer
            .create(create_input(), &Identity::user("alice"))
            .expect("create durable record")
            .name
    };
    let db = Arc::new(RedbObjectDb::open(&path).expect("reopen redb reader"));
    let reader = ledger_with_store(
        Arc::new(ObjectDbChangeRecordStore::new(db)),
        Arc::new(FixedClock::new(2_000)),
    );

    // Act
    let restored = reader.get(&name).expect("restore record after reopen");

    // Assert
    assert_eq!(restored.name, name);
    assert_eq!(restored.created_by.identity, "alice");
    assert_eq!(restored.status, ChangeRecordStatus::Draft);
}
