//! Postgres-backed [`ObjectDb`] — the multi-instance/server alternative to
//! the embedded redb default (design.md §4.5).
//!
//! ## Schema
//!
//! Three tables, created idempotently on `connect`:
//!
//! - `objects(kind INT NOT NULL, id BYTEA PRIMARY KEY, bytes BYTEA NOT NULL)` —
//!   content-addressed; the kind is informational only since `id` (the content
//!   hash) is globally unique. We still filter by kind in
//!   [`ObjectDb::list_objects`] / [`ObjectDb::delete_object`] /
//!   [`ObjectDb::get_object`] so callers see the same per-kind view they get
//!   from the in-memory and redb backends.
//! - `ops(repo TEXT, op_id BYTEA, op_bytes BYTEA, inserted_at BIGSERIAL,
//!   PRIMARY KEY(repo, op_id))` — per-repo op-log. `inserted_at` is a per-row
//!   monotonic sequence so [`ObjectDb::list_ops`] returns oldest→newest
//!   deterministically, matching the in-memory backend's semantics whenever a
//!   single writer streams ops in.
//! - `refs(repo TEXT, name TEXT, target BYTEA, PRIMARY KEY(repo, name))` —
//!   per-repo named refs.
//!
//! ## Sync ↔ async bridge
//!
//! The [`ObjectDb`] trait is sync (the broader schemahub-vcs surface — `Vcs`,
//! `Store`, the jj `Backend` impl — runs on a `pollster::block_on` substrate,
//! NOT a tokio runtime). sqlx is async-only on tokio, so we own a **dedicated
//! current-thread tokio runtime per `PgObjectDb` instance** and call
//! `runtime.block_on(async { … })` in each trait method. The runtime is parked
//! on a background thread so nested `block_on`s (e.g. when this code runs
//! from within a tokio worker — the server's tonic handlers — and then itself
//! tries to do sync DB I/O) don't panic with "cannot drive runtime from within
//! a runtime". This matches the existing `Store::block_on` pattern in
//! `repo.rs`, which spawns a separate runtime for jj's async backend.

use std::sync::Arc;
use std::thread;

use sha2::{Digest, Sha256};
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::Row;
use tokio::runtime::{Builder, Runtime};
use tokio::sync::oneshot;

use crate::object_db::{ObjectDb, ObjectDbError, ObjectDbResult, ObjectId, ObjectKind, OpId};

/// `sha256(tag ++ bytes)` — kind-tagged content hash. Identical to the redb
/// backend so any backend swap dedups against the same ids.
fn hash(kind: ObjectKind, bytes: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update([kind.tag()]);
    hasher.update(bytes);
    hasher.finalize().to_vec()
}

fn map_db<E: std::fmt::Display>(e: E) -> ObjectDbError {
    ObjectDbError::Backend(e.to_string())
}

/// Postgres-backed object store. Sync façade over an async sqlx `PgPool`,
/// bridged through a dedicated tokio runtime owned by this instance.
pub struct PgObjectDb {
    pool: PgPool,
    /// Background tokio runtime — `Arc` so async closures can hop onto it via
    /// `Handle::clone()`. Kept alive for the lifetime of this `PgObjectDb`.
    runtime: Arc<Runtime>,
}

impl std::fmt::Debug for PgObjectDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgObjectDb").finish_non_exhaustive()
    }
}

impl PgObjectDb {
    /// Open a connection pool against `url` and ensure the schema exists.
    ///
    /// Sync wrapper: spins up the dedicated runtime, runs `PgPool::connect`,
    /// and creates the three tables idempotently.
    pub fn connect(url: &str) -> ObjectDbResult<Self> {
        let runtime = Self::build_runtime()?;
        let pool = runtime
            .block_on(async { PgPoolOptions::new().max_connections(8).connect(url).await })
            .map_err(map_db)?;
        let db = Self {
            pool,
            runtime: Arc::new(runtime),
        };
        db.runtime
            .block_on(Self::init_schema(&db.pool))
            .map_err(map_db)?;
        Ok(db)
    }

    /// Construct a `PgObjectDb` over an externally-built `PgPool` (e.g. tests
    /// sharing one pool across many fixtures). Still owns its own runtime for
    /// the sync↔async bridge.
    pub fn with_pool(pool: PgPool) -> ObjectDbResult<Self> {
        let runtime = Self::build_runtime()?;
        let db = Self {
            pool,
            runtime: Arc::new(runtime),
        };
        db.runtime
            .block_on(Self::init_schema(&db.pool))
            .map_err(map_db)?;
        Ok(db)
    }

    /// Build the dedicated tokio runtime for this instance.
    ///
    /// Multi-thread with one worker is enough — sqlx's connection pool is the
    /// concurrency unit. We mainly want a runtime that isn't shared with the
    /// caller's tokio context so nested `block_on` is safe.
    fn build_runtime() -> ObjectDbResult<Runtime> {
        Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .thread_name("schemahub-pg-db")
            .build()
            .map_err(map_db)
    }

    /// Run an async block on this instance's dedicated runtime, even when the
    /// caller is already inside another tokio runtime (the server case).
    ///
    /// We always hop onto a fresh OS thread so `Runtime::block_on` is legal
    /// (a running tokio worker thread cannot call `block_on` on any runtime,
    /// including a different one). The cost is a thread spawn per DB call —
    /// acceptable for the schema-registry workload (low QPS, large objects).
    fn block_on<F, T>(&self, fut: F) -> T
    where
        F: std::future::Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let rt = self.runtime.clone();
        let (tx, rx) = oneshot::channel();
        thread::spawn(move || {
            let out = rt.block_on(fut);
            let _ = tx.send(out);
        });
        // `recv` on a std `oneshot` would block the caller's executor; we use
        // a tokio oneshot but block on it via `Runtime::block_on` only OFF the
        // caller's runtime, which is exactly what the spawned thread did. To
        // get the value back here, we use a plain std condvar-style wait via
        // `blocking_recv`.
        rx.blocking_recv()
            .expect("schemahub-pg-db worker thread panicked")
    }

    /// `CREATE TABLE IF NOT EXISTS` for the three tables, in one batch.
    async fn init_schema(pool: &PgPool) -> sqlx::Result<()> {
        // `objects`: kind is informational; `id` is the globally-unique content
        // hash. Storing kind lets `list_objects(kind)` enumerate per kind.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS objects (
                kind INTEGER NOT NULL,
                id BYTEA PRIMARY KEY,
                bytes BYTEA NOT NULL
            )",
        )
        .execute(pool)
        .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS objects_kind_idx ON objects (kind)")
            .execute(pool)
            .await?;

        // `ops`: per-repo content-addressed op-log. `inserted_at` is a
        // monotonic per-row sequence used to order list_ops deterministically.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS ops (
                repo TEXT NOT NULL,
                op_id BYTEA NOT NULL,
                op_bytes BYTEA NOT NULL,
                inserted_at BIGSERIAL NOT NULL,
                PRIMARY KEY (repo, op_id)
            )",
        )
        .execute(pool)
        .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS ops_repo_seq_idx ON ops (repo, inserted_at)")
            .execute(pool)
            .await?;

        // `refs`: per-repo named refs.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS refs (
                repo TEXT NOT NULL,
                name TEXT NOT NULL,
                target BYTEA NOT NULL,
                PRIMARY KEY (repo, name)
            )",
        )
        .execute(pool)
        .await?;

        Ok(())
    }
}

impl ObjectDb for PgObjectDb {
    fn put_object(&self, kind: ObjectKind, bytes: &[u8]) -> ObjectDbResult<ObjectId> {
        let id = ObjectId(hash(kind, bytes));
        let kind_i = kind.tag() as i32;
        let id_bytes = id.0.clone();
        let value = bytes.to_vec();
        let pool = self.pool.clone();
        self.block_on(async move {
            sqlx::query(
                "INSERT INTO objects (kind, id, bytes) VALUES ($1, $2, $3)
                 ON CONFLICT (id) DO NOTHING",
            )
            .bind(kind_i)
            .bind(&id_bytes)
            .bind(&value)
            .execute(&pool)
            .await
        })
        .map_err(map_db)?;
        Ok(id)
    }

    fn put_object_at(&self, kind: ObjectKind, id: &ObjectId, bytes: &[u8]) -> ObjectDbResult<()> {
        let kind_i = kind.tag() as i32;
        let id_bytes = id.0.clone();
        let value = bytes.to_vec();
        let pool = self.pool.clone();
        self.block_on(async move {
            sqlx::query(
                "INSERT INTO objects (kind, id, bytes) VALUES ($1, $2, $3)
                 ON CONFLICT (id) DO NOTHING",
            )
            .bind(kind_i)
            .bind(&id_bytes)
            .bind(&value)
            .execute(&pool)
            .await
        })
        .map_err(map_db)?;
        Ok(())
    }

    fn get_object(&self, kind: ObjectKind, id: &ObjectId) -> ObjectDbResult<Vec<u8>> {
        let kind_i = kind.tag() as i32;
        let id_bytes = id.0.clone();
        let pool = self.pool.clone();
        let row = self
            .block_on(async move {
                sqlx::query("SELECT bytes FROM objects WHERE kind = $1 AND id = $2")
                    .bind(kind_i)
                    .bind(&id_bytes)
                    .fetch_optional(&pool)
                    .await
            })
            .map_err(map_db)?;
        match row {
            Some(r) => Ok(r.try_get::<Vec<u8>, _>("bytes").map_err(map_db)?),
            None => Err(ObjectDbError::NotFound),
        }
    }

    fn has_object(&self, kind: ObjectKind, id: &ObjectId) -> ObjectDbResult<bool> {
        let kind_i = kind.tag() as i32;
        let id_bytes = id.0.clone();
        let pool = self.pool.clone();
        let row = self
            .block_on(async move {
                sqlx::query("SELECT 1 AS one FROM objects WHERE kind = $1 AND id = $2")
                    .bind(kind_i)
                    .bind(&id_bytes)
                    .fetch_optional(&pool)
                    .await
            })
            .map_err(map_db)?;
        Ok(row.is_some())
    }

    fn list_objects(&self, kind: ObjectKind) -> ObjectDbResult<Vec<ObjectId>> {
        let kind_i = kind.tag() as i32;
        let pool = self.pool.clone();
        let rows = self
            .block_on(async move {
                sqlx::query("SELECT id FROM objects WHERE kind = $1")
                    .bind(kind_i)
                    .fetch_all(&pool)
                    .await
            })
            .map_err(map_db)?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            out.push(ObjectId(r.try_get::<Vec<u8>, _>("id").map_err(map_db)?));
        }
        Ok(out)
    }

    fn delete_object(&self, kind: ObjectKind, id: &ObjectId) -> ObjectDbResult<()> {
        let kind_i = kind.tag() as i32;
        let id_bytes = id.0.clone();
        let pool = self.pool.clone();
        self.block_on(async move {
            sqlx::query("DELETE FROM objects WHERE kind = $1 AND id = $2")
                .bind(kind_i)
                .bind(&id_bytes)
                .execute(&pool)
                .await
        })
        .map_err(map_db)?;
        Ok(())
    }

    fn put_op(&self, repo: &str, op_bytes: &[u8]) -> ObjectDbResult<OpId> {
        // Op ids are content hashes of the operation record (matches redb/mem).
        let mut hasher = Sha256::new();
        hasher.update(b"op");
        hasher.update(op_bytes);
        let id = OpId(hasher.finalize().to_vec());
        let repo_s = repo.to_string();
        let id_bytes = id.0.clone();
        let value = op_bytes.to_vec();
        let pool = self.pool.clone();
        self.block_on(async move {
            sqlx::query(
                "INSERT INTO ops (repo, op_id, op_bytes) VALUES ($1, $2, $3)
                 ON CONFLICT (repo, op_id) DO NOTHING",
            )
            .bind(&repo_s)
            .bind(&id_bytes)
            .bind(&value)
            .execute(&pool)
            .await
        })
        .map_err(map_db)?;
        Ok(id)
    }

    fn put_op_at(&self, repo: &str, id: &OpId, op_bytes: &[u8]) -> ObjectDbResult<()> {
        let repo_s = repo.to_string();
        let id_bytes = id.0.clone();
        let value = op_bytes.to_vec();
        let pool = self.pool.clone();
        self.block_on(async move {
            sqlx::query(
                "INSERT INTO ops (repo, op_id, op_bytes) VALUES ($1, $2, $3)
                 ON CONFLICT (repo, op_id) DO NOTHING",
            )
            .bind(&repo_s)
            .bind(&id_bytes)
            .bind(&value)
            .execute(&pool)
            .await
        })
        .map_err(map_db)?;
        Ok(())
    }

    fn get_op(&self, repo: &str, id: &OpId) -> ObjectDbResult<Vec<u8>> {
        let repo_s = repo.to_string();
        let id_bytes = id.0.clone();
        let pool = self.pool.clone();
        let row = self
            .block_on(async move {
                sqlx::query("SELECT op_bytes FROM ops WHERE repo = $1 AND op_id = $2")
                    .bind(&repo_s)
                    .bind(&id_bytes)
                    .fetch_optional(&pool)
                    .await
            })
            .map_err(map_db)?;
        match row {
            Some(r) => Ok(r.try_get::<Vec<u8>, _>("op_bytes").map_err(map_db)?),
            None => Err(ObjectDbError::NotFound),
        }
    }

    fn list_ops(&self, repo: &str) -> ObjectDbResult<Vec<OpId>> {
        let repo_s = repo.to_string();
        let pool = self.pool.clone();
        let rows = self
            .block_on(async move {
                sqlx::query("SELECT op_id FROM ops WHERE repo = $1 ORDER BY inserted_at ASC")
                    .bind(&repo_s)
                    .fetch_all(&pool)
                    .await
            })
            .map_err(map_db)?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            out.push(OpId(r.try_get::<Vec<u8>, _>("op_id").map_err(map_db)?));
        }
        Ok(out)
    }

    fn set_ref(&self, repo: &str, name: &str, value: &[u8]) -> ObjectDbResult<()> {
        let repo_s = repo.to_string();
        let name_s = name.to_string();
        let val = value.to_vec();
        let pool = self.pool.clone();
        self.block_on(async move {
            sqlx::query(
                "INSERT INTO refs (repo, name, target) VALUES ($1, $2, $3)
                 ON CONFLICT (repo, name) DO UPDATE SET target = EXCLUDED.target",
            )
            .bind(&repo_s)
            .bind(&name_s)
            .bind(&val)
            .execute(&pool)
            .await
        })
        .map_err(map_db)?;
        Ok(())
    }

    fn get_ref(&self, repo: &str, name: &str) -> ObjectDbResult<Option<Vec<u8>>> {
        let repo_s = repo.to_string();
        let name_s = name.to_string();
        let pool = self.pool.clone();
        let row = self
            .block_on(async move {
                sqlx::query("SELECT target FROM refs WHERE repo = $1 AND name = $2")
                    .bind(&repo_s)
                    .bind(&name_s)
                    .fetch_optional(&pool)
                    .await
            })
            .map_err(map_db)?;
        match row {
            Some(r) => Ok(Some(r.try_get::<Vec<u8>, _>("target").map_err(map_db)?)),
            None => Ok(None),
        }
    }
}

// ── Integration tests ────────────────────────────────────────────────────────
//
// Gated by the `postgres-integration` feature so a plain `cargo test` (default
// features) does not require a Postgres instance. Run with:
//
//   SCHEMAHUB_TEST_POSTGRES_URL=postgres://cyuan:cyuan@docker.yuacx.com:5432/postgres \
//       cargo test -p schemahub-vcs --features postgres-integration

#[cfg(all(test, feature = "postgres-integration"))]
mod tests {
    use super::*;
    use sqlx::AssertSqlSafe;
    use uuid::Uuid;

    /// Fresh `PgObjectDb` bound to a UNIQUELY-named schema so parallel test
    /// runs don't collide. The schema is dropped on `Drop` so test runs leave
    /// no residue.
    struct TestDb {
        db: PgObjectDb,
        schema: String,
        /// Admin pool used to drop the schema in `Drop` — sits on the public
        /// schema so it doesn't depend on the test schema we're about to drop.
        admin_pool: PgPool,
        admin_runtime: Arc<Runtime>,
    }

    impl TestDb {
        fn new() -> Self {
            let url = std::env::var("SCHEMAHUB_TEST_POSTGRES_URL").expect(
                "SCHEMAHUB_TEST_POSTGRES_URL must be set to run postgres-integration tests",
            );
            // A unique schema per test isolates the three tables (objects/ops/refs)
            // and lets us drop them all in one statement.
            let schema = format!("shvcs_test_{}", Uuid::new_v4().simple());

            // Admin runtime + pool (separate from the PgObjectDb's runtime/pool):
            // used to CREATE/DROP SCHEMA and to set up the search_path.
            let admin_runtime = Arc::new(
                Builder::new_multi_thread()
                    .worker_threads(1)
                    .enable_all()
                    .thread_name("schemahub-pg-test-admin")
                    .build()
                    .expect("admin runtime"),
            );
            let admin_pool = admin_runtime
                .block_on(async { PgPoolOptions::new().max_connections(2).connect(&url).await })
                .expect("connect admin pool");
            // SAFETY (SqlSafeStr): `schema` is `shvcs_test_<uuid hex>` — fully
            // controlled by this test, ASCII alphanumeric + underscore, no
            // user input.
            let create_sql = format!("CREATE SCHEMA \"{schema}\"");
            admin_runtime
                .block_on(async {
                    sqlx::query(AssertSqlSafe(create_sql))
                        .execute(&admin_pool)
                        .await
                })
                .expect("create schema");

            // The PgObjectDb pool needs `search_path` to point at our schema so
            // its CREATE TABLE IF NOT EXISTS lands inside it. sqlx's PgPoolOptions
            // `after_connect` hook is the clean way to do that on every checkout.
            let schema_for_hook = schema.clone();
            let runtime = Arc::new(PgObjectDb::build_runtime().expect("build runtime"));
            let pool = runtime
                .block_on(async {
                    PgPoolOptions::new()
                        .max_connections(4)
                        .after_connect(move |conn, _meta| {
                            let s = schema_for_hook.clone();
                            Box::pin(async move {
                                // SAFETY (SqlSafeStr): same as above — `s` is the
                                // test-controlled schema identifier.
                                let sql = format!("SET search_path TO \"{s}\"");
                                sqlx::query(AssertSqlSafe(sql))
                                    .execute(&mut *conn)
                                    .await
                                    .map(|_| ())
                            })
                        })
                        .connect(&url)
                        .await
                })
                .expect("connect test pool");
            let db = PgObjectDb {
                pool,
                runtime: runtime.clone(),
            };
            db.runtime
                .block_on(PgObjectDb::init_schema(&db.pool))
                .expect("init schema");

            Self {
                db,
                schema,
                admin_pool,
                admin_runtime,
            }
        }
    }

    impl Drop for TestDb {
        fn drop(&mut self) {
            // SAFETY (SqlSafeStr): test-controlled schema identifier.
            let sql = format!("DROP SCHEMA IF EXISTS \"{}\" CASCADE", self.schema);
            let _ = self.admin_runtime.block_on(async {
                sqlx::query(AssertSqlSafe(sql))
                    .execute(&self.admin_pool)
                    .await
            });
        }
    }

    // ── ObjectDb contract — objects ──────────────────────────────────────────

    #[test]
    fn object_roundtrip_returns_identical_bytes() {
        // Arrange
        let t = TestDb::new();
        let bytes = b"hello declaration blob";

        // Act
        let id = t.db.put_object(ObjectKind::File, bytes).unwrap();
        let fetched = t.db.get_object(ObjectKind::File, &id).unwrap();

        // Assert
        assert_eq!(fetched, bytes);
        assert!(t.db.has_object(ObjectKind::File, &id).unwrap());
    }

    #[test]
    fn put_object_is_content_addressed_and_dedups() {
        // Arrange
        let t = TestDb::new();
        let bytes = b"same content";

        // Act
        let id1 = t.db.put_object(ObjectKind::File, bytes).unwrap();
        let id2 = t.db.put_object(ObjectKind::File, bytes).unwrap();

        // Assert
        assert_eq!(id1, id2);
        assert_eq!(t.db.list_objects(ObjectKind::File).unwrap().len(), 1);
    }

    #[test]
    fn get_object_missing_returns_not_found() {
        // Arrange
        let t = TestDb::new();
        let bogus = ObjectId(vec![0u8; 32]);

        // Act
        let result = t.db.get_object(ObjectKind::File, &bogus);

        // Assert
        assert!(matches!(result, Err(ObjectDbError::NotFound)));
    }

    #[test]
    fn list_objects_filters_by_kind() {
        // Arrange
        let t = TestDb::new();
        let file_id = t.db.put_object(ObjectKind::File, b"file blob").unwrap();
        let tree_id = t.db.put_object(ObjectKind::Tree, b"tree blob").unwrap();

        // Act
        let files = t.db.list_objects(ObjectKind::File).unwrap();
        let trees = t.db.list_objects(ObjectKind::Tree).unwrap();

        // Assert
        assert_eq!(files, vec![file_id]);
        assert_eq!(trees, vec![tree_id]);
    }

    #[test]
    fn delete_object_removes_it_and_is_idempotent() {
        // Arrange
        let t = TestDb::new();
        let id = t.db.put_object(ObjectKind::File, b"transient").unwrap();
        assert!(t.db.has_object(ObjectKind::File, &id).unwrap());

        // Act
        t.db.delete_object(ObjectKind::File, &id).unwrap();
        // Second delete must be a no-op.
        t.db.delete_object(ObjectKind::File, &id).unwrap();

        // Assert
        assert!(!t.db.has_object(ObjectKind::File, &id).unwrap());
    }

    #[test]
    fn put_object_at_writes_under_caller_id() {
        // Arrange
        let t = TestDb::new();
        let caller_id = ObjectId(vec![42u8; 32]);
        let bytes = b"jj-owned blob";

        // Act
        t.db.put_object_at(ObjectKind::Tree, &caller_id, bytes)
            .unwrap();
        let fetched = t.db.get_object(ObjectKind::Tree, &caller_id).unwrap();

        // Assert
        assert_eq!(fetched, bytes);
    }

    // ── ObjectDb contract — ops ──────────────────────────────────────────────

    #[test]
    fn put_op_then_get_op_roundtrips() {
        // Arrange
        let t = TestDb::new();
        let repo = "proj/repo";
        let bytes = b"op record bytes";

        // Act
        let id = t.db.put_op(repo, bytes).unwrap();
        let fetched = t.db.get_op(repo, &id).unwrap();

        // Assert
        assert_eq!(fetched, bytes);
    }

    #[test]
    fn list_ops_returns_inserts_in_oldest_to_newest_order() {
        // Arrange
        let t = TestDb::new();
        let repo = "proj/repo";

        // Act — three ops with distinct contents, inserted in order.
        let a = t.db.put_op(repo, b"op-a").unwrap();
        let b = t.db.put_op(repo, b"op-b").unwrap();
        let c = t.db.put_op(repo, b"op-c").unwrap();
        let ids = t.db.list_ops(repo).unwrap();

        // Assert — Postgres `inserted_at BIGSERIAL` preserves insertion order
        // for a single writer, the contract `Vcs::list_operations` relies on.
        assert_eq!(ids, vec![a, b, c]);
    }

    #[test]
    fn ops_are_scoped_per_repo() {
        // Arrange
        let t = TestDb::new();
        let _ = t.db.put_op("proj/one", b"op-1").unwrap();
        let _ = t.db.put_op("proj/two", b"op-2").unwrap();

        // Act
        let one = t.db.list_ops("proj/one").unwrap();
        let two = t.db.list_ops("proj/two").unwrap();

        // Assert
        assert_eq!(one.len(), 1);
        assert_eq!(two.len(), 1);
        assert_ne!(one, two);
    }

    // ── ObjectDb contract — refs ─────────────────────────────────────────────

    #[test]
    fn refs_set_and_get_roundtrip() {
        // Arrange
        let t = TestDb::new();
        let repo = "proj/repo";

        // Act
        t.db.set_ref(repo, "HEAD", b"deadbeef").unwrap();
        let value = t.db.get_ref(repo, "HEAD").unwrap();

        // Assert
        assert_eq!(value, Some(b"deadbeef".to_vec()));
    }

    #[test]
    fn refs_set_overwrites_previous_value() {
        // Arrange
        let t = TestDb::new();
        let repo = "proj/repo";
        t.db.set_ref(repo, "HEAD", b"v1").unwrap();

        // Act
        t.db.set_ref(repo, "HEAD", b"v2").unwrap();

        // Assert
        assert_eq!(t.db.get_ref(repo, "HEAD").unwrap(), Some(b"v2".to_vec()));
    }

    #[test]
    fn get_ref_missing_returns_none() {
        // Arrange
        let t = TestDb::new();

        // Act
        let value = t.db.get_ref("proj/repo", "no-such-ref").unwrap();

        // Assert
        assert_eq!(value, None);
    }
}
