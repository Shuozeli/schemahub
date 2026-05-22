mod helpers;
use helpers::*;
use schemahub_core::{CoreError, RepoConfig};
use schemahub_types::CompatibilityDirection;

// ── Project CRUD ──────────────────────────────────────────────────────────────

#[test]
fn create_and_get_project() {
    let core = make_core();
    core.create_project("acme", false).unwrap();

    let meta = core.get_project_meta("acme").unwrap().unwrap();
    assert_eq!(meta["name"], "acme");
    assert_eq!(meta["is_public"], false);
}

#[test]
fn create_project_duplicate_returns_already_exists() {
    let core = make_core();
    core.create_project("acme", false).unwrap();
    let err = core.create_project("acme", false).unwrap_err();
    assert!(matches!(err, CoreError::AlreadyExists(_)), "expected AlreadyExists, got {err:?}");
}

#[test]
fn get_nonexistent_project_returns_none() {
    let core = make_core();
    let meta = core.get_project_meta("ghost").unwrap();
    assert!(meta.is_none());
}

#[test]
fn list_projects_returns_all_with_prefix() {
    let core = make_core();
    core.create_project("alpha", false).unwrap();
    core.create_project("alpha-two", true).unwrap();
    core.create_project("beta", false).unwrap();

    let all = core.list_projects("").unwrap();
    assert_eq!(all.len(), 3);

    let alpha_only = core.list_projects("alpha").unwrap();
    assert_eq!(alpha_only.len(), 2);

    let beta_only = core.list_projects("beta").unwrap();
    assert_eq!(beta_only.len(), 1);
    assert_eq!(beta_only[0]["name"], "beta");
}

#[test]
fn delete_project_removes_it() {
    let core = make_core();
    core.create_project("temp", false).unwrap();
    core.delete_project("temp", false).unwrap();
    assert!(core.get_project_meta("temp").unwrap().is_none());
}

#[test]
fn delete_project_with_repos_requires_force() {
    let core = make_core();
    core.create_project("bigco", false).unwrap();
    core.initialize_repo("bigco", "schemas", &RepoConfig::default()).unwrap();

    let err = core.delete_project("bigco", false).unwrap_err();
    assert!(matches!(err, CoreError::InvalidArgument(_)));

    // force=true succeeds
    core.delete_project("bigco", true).unwrap();
    assert!(core.get_project_meta("bigco").unwrap().is_none());
}

#[test]
fn delete_nonexistent_project_returns_not_found() {
    let core = make_core();
    let err = core.delete_project("phantom", false).unwrap_err();
    assert!(matches!(err, CoreError::NotFound(_)));
}

// ── Repo CRUD ─────────────────────────────────────────────────────────────────

#[test]
fn initialize_repo_creates_branch_and_empty_commit() {
    let core = make_core();
    core.initialize_repo("acme", "schemas", &RepoConfig::default()).unwrap();

    // Branch should exist and point to a valid commit.
    let head = core.get_branch_head("acme", "schemas", "main").unwrap();
    assert!(!head.to_hex().is_empty());

    // Commit should be walkable.
    let commits = core.list_commits("acme", "schemas", Some("main"), None, 10).unwrap();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].1.message, "initial commit");
}

#[test]
fn initialize_repo_duplicate_returns_already_exists() {
    let core = make_core();
    core.initialize_repo("acme", "schemas", &RepoConfig::default()).unwrap();
    let err = core.initialize_repo("acme", "schemas", &RepoConfig::default()).unwrap_err();
    assert!(matches!(err, CoreError::AlreadyExists(_)));
}

#[test]
fn initialize_repo_custom_default_branch() {
    let config = RepoConfig {
        default_branch: "trunk".to_string(),
        ..RepoConfig::default()
    };
    let core = make_core();
    core.initialize_repo("acme", "lib", &config).unwrap();
    core.get_branch_head("acme", "lib", "trunk").unwrap();

    // "main" should NOT exist.
    let err = core.get_branch_head("acme", "lib", "main").unwrap_err();
    assert!(matches!(err, CoreError::NotFound(_)));
}

#[test]
fn list_repos_returns_repos_in_project() {
    let core = make_core();
    core.initialize_repo("acme", "schemas", &RepoConfig::default()).unwrap();
    core.initialize_repo("acme", "events", &RepoConfig::default()).unwrap();
    core.initialize_repo("other", "stuff", &RepoConfig::default()).unwrap();

    let repos = core.list_repos("acme", "").unwrap();
    let names: Vec<&str> = repos.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"schemas"), "missing 'schemas': {names:?}");
    assert!(names.contains(&"events"), "missing 'events': {names:?}");
    assert_eq!(repos.len(), 2, "should not include 'other' project repos");
}

#[test]
fn list_repos_prefix_filter() {
    let core = make_core();
    core.initialize_repo("acme", "api-v1", &RepoConfig::default()).unwrap();
    core.initialize_repo("acme", "api-v2", &RepoConfig::default()).unwrap();
    core.initialize_repo("acme", "internal", &RepoConfig::default()).unwrap();

    let api_repos = core.list_repos("acme", "api").unwrap();
    assert_eq!(api_repos.len(), 2);
}

#[test]
fn delete_repo_removes_config_and_refs() {
    let core = make_core();
    core.initialize_repo("acme", "temp", &RepoConfig::default()).unwrap();
    core.get_branch_head("acme", "temp", "main").unwrap(); // sanity

    core.delete_repo("acme", "temp", true).unwrap();

    // Branch ref should be gone.
    let err = core.get_branch_head("acme", "temp", "main").unwrap_err();
    assert!(matches!(err, CoreError::NotFound(_)));

    // Repo should not appear in listing.
    let repos = core.list_repos("acme", "").unwrap();
    assert!(repos.is_empty());
}

#[test]
fn delete_nonexistent_repo_returns_not_found() {
    let core = make_core();
    let err = core.delete_repo("acme", "ghost", false).unwrap_err();
    assert!(matches!(err, CoreError::NotFound(_)));
}

#[test]
fn get_and_set_repo_config() {
    let core = make_core();
    core.initialize_repo("acme", "schemas", &RepoConfig::default()).unwrap();

    let mut cfg = core.get_repo_config("acme", "schemas").unwrap();
    assert_eq!(cfg.compatibility_direction, CompatibilityDirection::Backward);

    cfg.compatibility_direction = CompatibilityDirection::Full;
    cfg.protected_branches.push("release/*".to_string());
    core.set_repo_config("acme", "schemas", &cfg).unwrap();

    let reloaded = core.get_repo_config("acme", "schemas").unwrap();
    assert_eq!(reloaded.compatibility_direction, CompatibilityDirection::Full);
    assert!(reloaded.protected_branches.contains(&"release/*".to_string()));
}

// ── Member management ─────────────────────────────────────────────────────────

#[test]
fn add_and_list_members() {
    let core = make_core();
    core.create_project("acme", false).unwrap();

    core.add_member("acme", "alice", "owner").unwrap();
    core.add_member("acme", "bob", "writer").unwrap();

    let members = core.list_members("acme").unwrap();
    assert_eq!(members.len(), 2);
    let member_map: std::collections::HashMap<_, _> = members.into_iter().collect();
    assert_eq!(member_map["alice"], "owner");
    assert_eq!(member_map["bob"], "writer");
}

#[test]
fn remove_member() {
    let core = make_core();
    core.create_project("acme", false).unwrap();
    core.add_member("acme", "carol", "reader").unwrap();
    core.remove_member("acme", "carol").unwrap();

    let members = core.list_members("acme").unwrap();
    assert!(members.is_empty());
}

#[test]
fn update_member_role_by_overwrite() {
    let core = make_core();
    core.create_project("acme", false).unwrap();
    core.add_member("acme", "dave", "reader").unwrap();
    core.add_member("acme", "dave", "maintainer").unwrap(); // overwrite

    let members = core.list_members("acme").unwrap();
    let m: std::collections::HashMap<_, _> = members.into_iter().collect();
    assert_eq!(m["dave"], "maintainer");
}
