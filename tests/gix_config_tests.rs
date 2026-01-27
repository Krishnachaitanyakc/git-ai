use git_ai::git::repository::find_repository;
use serial_test::serial;
use std::env;
use std::process::Command;
use tempfile::TempDir;

/// Helper to create a git repo for testing
fn create_test_repo() -> (TempDir, git_ai::git::repository::Repository) {
    let temp_dir = TempDir::new().unwrap();
    let repo_dir = temp_dir.path().join("repo");
    std::fs::create_dir(&repo_dir).unwrap();

    Command::new("git")
        .arg("init")
        .current_dir(&repo_dir)
        .output()
        .expect("failed to init repo");

    let args = vec!["-C".to_string(), repo_dir.to_str().unwrap().to_string()];
    let repo = find_repository(&args).expect("Failed to find repo");

    (temp_dir, repo)
}

/// Helper to set git config in a repo
fn set_git_config(repo_path: &std::path::Path, key: &str, value: &str) {
    Command::new("git")
        .args(["config", key, value])
        .current_dir(repo_path)
        .output()
        .expect("failed to set config");
}

/// Helper to get git config via CLI (for comparison)
fn get_git_config_cli(repo_path: &std::path::Path, key: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["config", "--get", key])
        .current_dir(repo_path)
        .output()
        .expect("failed to get config");

    if output.status.success() {
        Some(String::from_utf8(output.stdout).unwrap().trim().to_string())
    } else {
        None
    }
}

/// Helper to get git config regexp via CLI (for comparison)
fn get_git_config_regexp_cli(
    repo_path: &std::path::Path,
    pattern: &str,
) -> std::collections::HashMap<String, String> {
    let output = Command::new("git")
        .args(["config", "--get-regexp", pattern])
        .current_dir(repo_path)
        .output()
        .expect("failed to get config");

    let mut result = std::collections::HashMap::new();
    if output.status.success() {
        let stdout = String::from_utf8(output.stdout).unwrap();
        for line in stdout.lines() {
            if let Some((key, value)) = line.split_once(' ') {
                result.insert(key.to_string(), value.to_string());
            }
        }
    }
    result
}

#[test]
#[serial]
fn test_config_get_str_reads_global() {
    let temp_dir = TempDir::new().unwrap();

    // Setup global config with Mixed Case
    let global_config_path = temp_dir.path().join("global_gitconfig");
    std::fs::write(&global_config_path, "[User]\nName = MixedCaseUser\n").unwrap();

    // Setup repo
    let repo_dir = temp_dir.path().join("repo");
    std::fs::create_dir(&repo_dir).unwrap();

    // Init git repo
    Command::new("git")
        .arg("init")
        .current_dir(&repo_dir)
        .output()
        .expect("failed to init repo");

    // Use GIT_CONFIG_GLOBAL instead of modifying HOME - safer for parallel tests
    let old_config_global = env::var("GIT_CONFIG_GLOBAL").ok();
    unsafe {
        env::set_var("GIT_CONFIG_GLOBAL", &global_config_path);
    }

    let args = vec!["-C".to_string(), repo_dir.to_str().unwrap().to_string()];
    let repo = find_repository(&args).expect("Failed to find repo");

    // Test 1: config_get_str (lookup)
    let result = repo.config_get_str("user.name");

    // Test 2: config_get_regexp
    // We use a pattern that expects lowercase, which is typical for git config keys usage
    let regexp_result = repo.config_get_regexp(r"^user\.name$");

    // Restore GIT_CONFIG_GLOBAL
    unsafe {
        if let Some(g) = old_config_global {
            env::set_var("GIT_CONFIG_GLOBAL", g);
        } else {
            env::remove_var("GIT_CONFIG_GLOBAL");
        }
    }

    assert!(
        result.is_ok(),
        "config_get_str returned error: {:?}",
        result.err()
    );
    let value = result.unwrap();
    assert_eq!(
        value,
        Some("MixedCaseUser".to_string()),
        "Did not read global config with mixed case lookup"
    );

    assert!(regexp_result.is_ok(), "config_get_regexp failed");
    let map = regexp_result.unwrap();

    // Check if we got any match
    if map.is_empty() {
        println!("Map was empty!");
    } else {
        println!("Map keys: {:?}", map.keys());
    }

    assert_eq!(map.get("user.name"), Some(&"MixedCaseUser".to_string()));
}

// ============================================================================
// Tests for config_get_str behavior parity with git CLI
// ============================================================================

#[test]
#[serial]
fn test_config_get_str_simple_value() {
    let (temp_dir, repo) = create_test_repo();
    let repo_path = temp_dir.path().join("repo");

    set_git_config(&repo_path, "user.name", "TestUser");
    set_git_config(&repo_path, "user.email", "test@example.com");

    // Verify via gix-config
    let name = repo.config_get_str("user.name").unwrap();
    let email = repo.config_get_str("user.email").unwrap();

    // Verify matches git CLI
    assert_eq!(name, get_git_config_cli(&repo_path, "user.name"));
    assert_eq!(email, get_git_config_cli(&repo_path, "user.email"));
}

#[test]
#[serial]
fn test_config_get_str_missing_key_returns_none() {
    let (_temp_dir, repo) = create_test_repo();

    // Non-existent key should return None (same as git config --get exit code 1)
    let result = repo.config_get_str("nonexistent.key").unwrap();
    assert_eq!(result, None);
}

#[test]
#[serial]
fn test_config_get_str_subsection_value() {
    let (temp_dir, repo) = create_test_repo();
    let repo_path = temp_dir.path().join("repo");

    // Set a config with subsection (common pattern: remote.origin.url)
    set_git_config(&repo_path, "remote.origin.url", "https://github.com/test/repo.git");

    let url = repo.config_get_str("remote.origin.url").unwrap();
    let cli_url = get_git_config_cli(&repo_path, "remote.origin.url");

    assert_eq!(url, cli_url);
    assert_eq!(url, Some("https://github.com/test/repo.git".to_string()));
}

#[test]
#[serial]
fn test_config_get_str_boolean_values() {
    let (temp_dir, repo) = create_test_repo();
    let repo_path = temp_dir.path().join("repo");

    set_git_config(&repo_path, "core.autocrlf", "true");
    set_git_config(&repo_path, "pull.rebase", "false");

    let autocrlf = repo.config_get_str("core.autocrlf").unwrap();
    let rebase = repo.config_get_str("pull.rebase").unwrap();

    assert_eq!(autocrlf, get_git_config_cli(&repo_path, "core.autocrlf"));
    assert_eq!(rebase, get_git_config_cli(&repo_path, "pull.rebase"));
}

#[test]
#[serial]
fn test_config_get_str_special_characters() {
    let (temp_dir, repo) = create_test_repo();
    let repo_path = temp_dir.path().join("repo");

    // Test values with special characters
    set_git_config(&repo_path, "user.name", "Test User <test@example.com>");
    set_git_config(&repo_path, "alias.lg", "log --oneline --graph");

    let name = repo.config_get_str("user.name").unwrap();
    let alias = repo.config_get_str("alias.lg").unwrap();

    assert_eq!(name, get_git_config_cli(&repo_path, "user.name"));
    assert_eq!(alias, get_git_config_cli(&repo_path, "alias.lg"));
}

// ============================================================================
// Tests for config_get_regexp behavior parity with git CLI
// ============================================================================

#[test]
#[serial]
fn test_config_get_regexp_simple_pattern() {
    let (temp_dir, repo) = create_test_repo();
    let repo_path = temp_dir.path().join("repo");

    set_git_config(&repo_path, "user.name", "TestUser");
    set_git_config(&repo_path, "user.email", "test@example.com");

    // Match all user.* keys
    let result = repo.config_get_regexp(r"^user\.").unwrap();
    let cli_result = get_git_config_regexp_cli(&repo_path, r"^user\.");

    assert_eq!(result.len(), cli_result.len());
    assert_eq!(result.get("user.name"), cli_result.get("user.name"));
    assert_eq!(result.get("user.email"), cli_result.get("user.email"));
}

#[test]
#[serial]
fn test_config_get_regexp_no_matches() {
    let (_temp_dir, repo) = create_test_repo();

    // Pattern that matches nothing should return empty HashMap
    let result = repo.config_get_regexp(r"^nonexistent\.").unwrap();
    assert!(result.is_empty());
}

#[test]
#[serial]
fn test_config_get_regexp_exact_key() {
    let (temp_dir, repo) = create_test_repo();
    let repo_path = temp_dir.path().join("repo");

    set_git_config(&repo_path, "pull.rebase", "true");
    set_git_config(&repo_path, "rebase.autoStash", "true");

    // Test exact pattern match (used in fetch_hooks.rs)
    let result = repo.config_get_regexp(r"^(pull\.rebase|rebase\.autostash)$").unwrap();

    // Should find both keys
    assert!(
        result.contains_key("pull.rebase"),
        "Expected pull.rebase, got keys: {:?}",
        result.keys()
    );
    assert!(
        result.contains_key("rebase.autostash"),
        "Expected rebase.autostash, got keys: {:?}",
        result.keys()
    );
}

#[test]
#[serial]
fn test_config_get_regexp_with_subsections() {
    let (temp_dir, repo) = create_test_repo();
    let repo_path = temp_dir.path().join("repo");

    set_git_config(&repo_path, "remote.origin.url", "https://github.com/test/repo.git");
    set_git_config(&repo_path, "remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*");
    set_git_config(&repo_path, "remote.upstream.url", "https://github.com/upstream/repo.git");

    // Match all remote.*.url keys
    let result = repo.config_get_regexp(r"^remote\..*\.url$").unwrap();

    assert_eq!(result.len(), 2);
    assert!(result.contains_key("remote.origin.url"));
    assert!(result.contains_key("remote.upstream.url"));
}

#[test]
#[serial]
fn test_config_get_regexp_case_insensitive_keys() {
    let (temp_dir, repo) = create_test_repo();
    let repo_path = temp_dir.path().join("repo");

    // Git config keys are case-insensitive
    set_git_config(&repo_path, "Core.AutoCRLF", "true");

    // Our implementation normalizes to lowercase
    let result = repo.config_get_regexp(r"^core\.autocrlf$").unwrap();

    assert!(
        result.contains_key("core.autocrlf"),
        "Expected core.autocrlf in lowercase, got: {:?}",
        result.keys()
    );
}

#[test]
#[serial]
fn test_config_get_regexp_partial_match() {
    let (temp_dir, repo) = create_test_repo();
    let repo_path = temp_dir.path().join("repo");

    set_git_config(&repo_path, "alias.st", "status");
    set_git_config(&repo_path, "alias.co", "checkout");
    set_git_config(&repo_path, "alias.ci", "commit");

    // Match all alias.* keys
    let result = repo.config_get_regexp(r"alias\.").unwrap();
    let cli_result = get_git_config_regexp_cli(&repo_path, r"alias\.");

    // Verify we get the same keys as git CLI (may include global aliases)
    assert_eq!(
        result.len(),
        cli_result.len(),
        "gix-config returned {:?}, git CLI returned {:?}",
        result.keys().collect::<Vec<_>>(),
        cli_result.keys().collect::<Vec<_>>()
    );

    // Verify the specific values we set
    assert_eq!(result.get("alias.st"), Some(&"status".to_string()));
    assert_eq!(result.get("alias.co"), Some(&"checkout".to_string()));
    assert_eq!(result.get("alias.ci"), Some(&"commit".to_string()));
}
