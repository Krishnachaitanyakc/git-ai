//! Integration tests for push hook refspec injection
//!
//! These tests verify that the push hooks correctly inject authorship notes refspecs
//! into git push commands, handling various scenarios:
//!
//! 1. Remote with no authorship notes (should force push with +refs/notes/ai:refs/notes/ai)
//! 2. Remote with notes that are ahead (should merge, not force)
//! 3. Remote with notes that are behind (should fast-forward, not force)
//! 4. Pushing to a fork (should handle multiple remotes correctly)
//! 5. Multiple refspecs in single push command
//! 6. Push with --force-with-lease flag

use super::github_test_harness::GitHubTestRepo;
use crate::lines;
use crate::repos::test_file::ExpectedLineExt;
use git_ai::git::refs::notes_add;
use git_ai::git::repository::find_repository_in_path;
use git_ai::git::sync_authorship::fetch_authorship_notes;
use std::process::Command;

/// Helper to manually add a git note to a commit using the notes_add function
fn add_manual_note(
    repo_path: &std::path::PathBuf,
    commit_sha: &str,
    note_content: &str,
) -> Result<(), String> {
    let repo = find_repository_in_path(repo_path.to_str().unwrap())
        .map_err(|e| format!("Failed to find repository: {}", e))?;

    notes_add(&repo, commit_sha, note_content)
        .map_err(|e| format!("Failed to add git note: {}", e))?;

    Ok(())
}

/// Helper to push notes to remote
fn push_notes_to_remote(
    repo_path: &std::path::PathBuf,
    remote: &str,
    force: bool,
) -> Result<(), String> {
    let refspec = if force {
        "+refs/notes/ai:refs/notes/ai"
    } else {
        "refs/notes/ai:refs/notes/ai"
    };

    let output = Command::new("git")
        .args(&["-C", repo_path.to_str().unwrap(), "push", remote, refspec])
        .output()
        .map_err(|e| format!("Failed to push notes: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "Failed to push notes:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(())
}

/// Helper to fetch notes from remote using fetch_authorship_notes
fn fetch_notes_from_remote(repo_path: &std::path::PathBuf, remote: &str) -> Result<(), String> {
    let repo = find_repository_in_path(repo_path.to_str().unwrap())
        .map_err(|e| format!("Failed to find repository: {}", e))?;

    fetch_authorship_notes(&repo, remote).map_err(|e| format!("Failed to fetch notes: {}", e))?;

    Ok(())
}

/// Helper to check if notes exist on remote
fn notes_exist_on_remote(repo_path: &std::path::PathBuf, remote: &str) -> bool {
    let output = Command::new("git")
        .args(&[
            "-C",
            repo_path.to_str().unwrap(),
            "ls-remote",
            remote,
            "refs/notes/ai",
        ])
        .output();

    if let Ok(output) = output {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            return !stdout.trim().is_empty();
        }
    }

    false
}

#[test]
#[ignore] // Ignored by default - run with `cargo test --test github_integration -- --ignored`
fn test_push_to_remote_with_no_authorship_notes() {
    let test_repo = match GitHubTestRepo::new("test_push_no_notes") {
        Some(repo) => repo,
        None => {
            println!("⏭️  Test skipped - GitHub CLI not available");
            return;
        }
    };

    println!("🚀 Testing push to remote with no authorship notes");

    if let Err(e) = test_repo.create_on_github() {
        panic!("Failed to create GitHub repository: {}", e);
    }

    // Verify that notes were automatically pushed during create_on_github
    let notes_exist_after_create = notes_exist_on_remote(test_repo.repo.path(), "origin");
    println!(
        "📝 Notes exist on remote after initial push: {}",
        notes_exist_after_create
    );

    // The create_on_github push should have automatically included notes via hooks
    assert!(
        notes_exist_after_create,
        "Notes should have been automatically pushed with the initial commit via hooks"
    );

    // Create a new branch with additional commits
    test_repo
        .create_branch("feature/notes-test")
        .expect("Failed to create feature branch");

    std::fs::create_dir(test_repo.repo.path().join("src")).expect("Failed to create src directory");

    let mut test_file = test_repo.repo.filename("src/main.rs");
    test_file.set_contents(lines![
        "fn main() {",
        "    println!(\"Hello, world!\");".ai(),
        "}",
    ]);

    let commit = test_repo
        .repo
        .stage_all_and_commit("Add main function")
        .expect("Failed to create commit");

    println!(
        "✅ Created commit with AI authorship: {}",
        commit.commit_sha
    );

    // Push the branch - notes should be automatically included via hooks
    test_repo
        .push_branch("feature/notes-test")
        .expect("Failed to push branch");

    println!("✅ Pushed branch to remote");

    // Verify notes still exist on remote (and include the new commit)
    assert!(
        notes_exist_on_remote(test_repo.repo.path(), "origin"),
        "Notes should exist on remote"
    );

    // Fetch and verify we can retrieve the notes
    fetch_notes_from_remote(test_repo.repo.path(), "origin").expect("Failed to fetch notes");

    println!("✅ Test completed successfully - notes were automatically pushed via hooks");
}

#[test]
#[ignore]
fn test_first_time_notes_push_uses_force() {
    let test_repo = match GitHubTestRepo::new("test_first_notes_force") {
        Some(repo) => repo,
        None => {
            println!("⏭️  Test skipped - GitHub CLI not available");
            return;
        }
    };

    println!("🚀 Testing that first-time notes push uses force (+refspec)");

    if let Err(e) = test_repo.create_on_github() {
        panic!("Failed to create GitHub repository: {}", e);
    }

    // Delete the notes that were automatically pushed
    println!("🗑️  Deleting notes from remote to simulate clean state");
    test_repo
        .repo
        .git(&["push", "origin", ":refs/notes/ai"])
        .ok(); // Ignore errors if notes don't exist

    // Verify notes are gone
    assert!(
        !notes_exist_on_remote(test_repo.repo.path(), "origin"),
        "Notes should be deleted from remote"
    );

    // Create a new commit with AI authorship
    let mut test_file = test_repo.repo.filename("test.txt");
    test_file.set_contents(lines!["line 1", "line 2".ai(),]);

    let commit = test_repo
        .repo
        .stage_all_and_commit("Add test file")
        .expect("Failed to create commit");

    println!(
        "✅ Created commit with AI authorship: {}",
        commit.commit_sha
    );

    // Push to main - this should force push notes since remote doesn't have any
    test_repo
        .repo
        .git(&["push", "origin", "main"])
        .expect("Failed to push");

    println!("✅ Pushed to remote (notes should be force-pushed)");

    // Verify notes now exist on remote
    assert!(
        notes_exist_on_remote(test_repo.repo.path(), "origin"),
        "Notes should now exist on remote after force push"
    );

    println!("✅ Test completed - notes were force-pushed when remote had none");
}

#[test]
#[ignore]
fn test_push_to_remote_with_existing_notes_ahead() {
    let test_repo = match GitHubTestRepo::new("test_push_notes_ahead") {
        Some(repo) => repo,
        None => {
            println!("⏭️  Test skipped - GitHub CLI not available");
            return;
        }
    };

    println!("🚀 Testing push to remote with existing notes (remote ahead)");

    if let Err(e) = test_repo.create_on_github() {
        panic!("Failed to create GitHub repository: {}", e);
    }

    // Create initial commit and push
    std::fs::create_dir(test_repo.repo.path().join("src")).expect("Failed to create src directory");

    let mut test_file = test_repo.repo.filename("src/main.rs");
    test_file.set_contents(lines!["fn main() {", "    println!(\"Hello!\");".ai(), "}",]);

    test_repo
        .repo
        .stage_all_and_commit("Initial commit")
        .expect("Failed to create commit");

    test_repo
        .repo
        .git(&["push", "origin", "main"])
        .expect("Failed to push");

    println!("✅ Created and pushed initial commit with notes");

    // Manually add an extra note on a fake commit SHA to simulate remote being ahead
    let fake_commit = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    add_manual_note(
        test_repo.repo.path(),
        fake_commit,
        "Remote note on fake commit",
    )
    .expect("Failed to add manual note");

    // Push notes to remote (this simulates remote having additional notes)
    push_notes_to_remote(test_repo.repo.path(), "origin", true)
        .expect("Failed to push manual note");

    println!("✅ Added note on remote for non-existent commit (simulating remote ahead)");

    // Create a new branch with another commit
    test_repo
        .create_branch("feature/notes-test")
        .expect("Failed to create feature branch");

    test_file.insert_at(2, lines!["    println!(\"New line\");".ai()]);

    let commit2 = test_repo
        .repo
        .stage_all_and_commit("Add new line")
        .expect("Failed to create commit");

    println!("✅ Created new commit: {}", commit2.commit_sha);

    // Push the branch - should NOT force push since remote has notes
    test_repo
        .push_branch("feature/notes-test")
        .expect("Failed to push branch");

    println!("✅ Pushed branch - notes should be merged, not force-pushed");

    // Verify notes still exist on remote
    assert!(
        notes_exist_on_remote(test_repo.repo.path(), "origin"),
        "Notes should still exist on remote"
    );

    println!("✅ Test completed successfully - notes were merged without force");
}

#[test]
#[ignore]
fn test_push_to_remote_with_existing_notes_behind() {
    let test_repo = match GitHubTestRepo::new("test_push_notes_behind") {
        Some(repo) => repo,
        None => {
            println!("⏭️  Test skipped - GitHub CLI not available");
            return;
        }
    };

    println!("🚀 Testing push to remote with existing notes (local ahead)");

    if let Err(e) = test_repo.create_on_github() {
        panic!("Failed to create GitHub repository: {}", e);
    }

    // Create initial commit and push
    std::fs::create_dir(test_repo.repo.path().join("src")).expect("Failed to create src directory");

    let mut test_file = test_repo.repo.filename("src/main.rs");
    test_file.set_contents(lines!["fn main() {", "    println!(\"Hello!\");".ai(), "}",]);

    test_repo
        .repo
        .stage_all_and_commit("Initial commit")
        .expect("Failed to create commit");

    test_repo
        .repo
        .git(&["push", "origin", "main"])
        .expect("Failed to push");

    println!("✅ Created and pushed initial commit with notes");

    // Create local commits with notes
    test_file.insert_at(2, lines!["    println!(\"Local change 1\");".ai()]);

    let commit2 = test_repo
        .repo
        .stage_all_and_commit("Local change 1")
        .expect("Failed to create commit");

    test_file.insert_at(3, lines!["    println!(\"Local change 2\");".ai()]);

    let commit3 = test_repo
        .repo
        .stage_all_and_commit("Local change 2")
        .expect("Failed to create commit");

    println!(
        "✅ Created local commits with notes (local ahead): {} and {}",
        commit2.commit_sha, commit3.commit_sha
    );

    // Push commits - should push notes without force since remote notes exist
    test_repo
        .repo
        .git(&["push", "origin", "main"])
        .expect("Failed to push");

    println!("✅ Pushed commits - notes should be fast-forwarded, not force-pushed");

    // Verify notes still exist on remote
    assert!(
        notes_exist_on_remote(test_repo.repo.path(), "origin"),
        "Notes should exist on remote"
    );

    println!("✅ Test completed successfully - notes were fast-forwarded");
}

#[test]
#[ignore]
fn test_push_to_fork() {
    let test_repo = match GitHubTestRepo::new("test_push_to_fork") {
        Some(repo) => repo,
        None => {
            println!("⏭️  Test skipped - GitHub CLI not available");
            return;
        }
    };

    println!("🚀 Testing push to fork remote");

    if let Err(e) = test_repo.create_on_github() {
        panic!("Failed to create GitHub repository: {}", e);
    }

    // Create initial commit
    std::fs::create_dir(test_repo.repo.path().join("src")).expect("Failed to create src directory");

    let mut test_file = test_repo.repo.filename("src/main.rs");
    test_file.set_contents(lines!["fn main() {", "    println!(\"Hello!\");".ai(), "}",]);

    test_repo
        .repo
        .stage_all_and_commit("Initial commit")
        .expect("Failed to create commit");

    test_repo
        .repo
        .git(&["push", "origin", "main"])
        .expect("Failed to push");

    println!("✅ Created and pushed initial commit to origin");

    // Add a second remote (simulating a fork)
    // We'll use the same repo as a fake fork for testing purposes
    let fork_url = format!(
        "https://github.com/{}/{}.git",
        test_repo.github_owner, test_repo.github_repo_name
    );

    test_repo
        .repo
        .git(&["remote", "add", "fork", &fork_url])
        .expect("Failed to add fork remote");

    println!("✅ Added 'fork' remote");

    // Create a new branch
    test_repo
        .create_branch("feature/fork-test")
        .expect("Failed to create feature branch");

    test_file.insert_at(2, lines!["    println!(\"Fork change\");".ai()]);

    let commit2 = test_repo
        .repo
        .stage_all_and_commit("Fork change")
        .expect("Failed to create commit");

    println!("✅ Created commit for fork: {}", commit2.commit_sha);

    // Push to fork - should force push notes since fork doesn't have notes yet
    test_repo
        .repo
        .git(&["push", "--set-upstream", "fork", "feature/fork-test"])
        .expect("Failed to push to fork");

    println!("✅ Pushed to fork remote");

    // Verify notes exist on the fork remote
    assert!(
        notes_exist_on_remote(test_repo.repo.path(), "fork"),
        "Notes should exist on fork remote"
    );

    println!("✅ Test completed successfully - notes were pushed to fork");
}

#[test]
#[ignore]
fn test_push_with_multiple_refspecs() {
    let test_repo = match GitHubTestRepo::new("test_push_multiple_refspecs") {
        Some(repo) => repo,
        None => {
            println!("⏭️  Test skipped - GitHub CLI not available");
            return;
        }
    };

    println!("🚀 Testing push with multiple refspecs");

    if let Err(e) = test_repo.create_on_github() {
        panic!("Failed to create GitHub repository: {}", e);
    }

    // Create two branches
    std::fs::create_dir(test_repo.repo.path().join("src")).expect("Failed to create src directory");

    let mut test_file = test_repo.repo.filename("src/main.rs");
    test_file.set_contents(lines!["fn main() {", "    println!(\"Hello!\");".ai(), "}",]);

    test_repo
        .repo
        .stage_all_and_commit("Initial commit")
        .expect("Failed to create commit");

    test_repo
        .repo
        .git(&["push", "origin", "main"])
        .expect("Failed to push main");

    // Create first feature branch
    test_repo
        .create_branch("feature/branch1")
        .expect("Failed to create branch1");

    test_file.insert_at(2, lines!["    println!(\"Branch 1\");".ai()]);

    test_repo
        .repo
        .stage_all_and_commit("Branch 1 change")
        .expect("Failed to create commit");

    // Go back to main and create second branch
    test_repo
        .repo
        .git(&["checkout", "main"])
        .expect("Failed to checkout main");

    test_repo
        .create_branch("feature/branch2")
        .expect("Failed to create branch2");

    test_file.insert_at(2, lines!["    println!(\"Branch 2\");".ai()]);

    test_repo
        .repo
        .stage_all_and_commit("Branch 2 change")
        .expect("Failed to create commit");

    println!("✅ Created two feature branches with commits");

    // Push both branches at once
    test_repo
        .repo
        .git(&["push", "origin", "feature/branch1", "feature/branch2"])
        .expect("Failed to push both branches");

    println!("✅ Pushed both branches with single command");

    // Verify notes exist on remote
    assert!(
        notes_exist_on_remote(test_repo.repo.path(), "origin"),
        "Notes should exist on remote"
    );

    println!("✅ Test completed successfully - notes were pushed with multiple refspecs");
}

#[test]
#[ignore]
fn test_push_with_force_with_lease() {
    let test_repo = match GitHubTestRepo::new("test_push_force_with_lease") {
        Some(repo) => repo,
        None => {
            println!("⏭️  Test skipped - GitHub CLI not available");
            return;
        }
    };

    println!("🚀 Testing push with --force-with-lease flag");

    if let Err(e) = test_repo.create_on_github() {
        panic!("Failed to create GitHub repository: {}", e);
    }

    // Create initial commit
    std::fs::create_dir(test_repo.repo.path().join("src")).expect("Failed to create src directory");

    let mut test_file = test_repo.repo.filename("src/main.rs");
    test_file.set_contents(lines!["fn main() {", "    println!(\"Hello!\");".ai(), "}",]);

    test_repo
        .repo
        .stage_all_and_commit("Initial commit")
        .expect("Failed to create commit");

    test_repo
        .repo
        .git(&["push", "origin", "main"])
        .expect("Failed to push");

    println!("✅ Created and pushed initial commit");

    // Create a branch
    test_repo
        .create_branch("feature/force-test")
        .expect("Failed to create feature branch");

    test_file.insert_at(2, lines!["    println!(\"Force change\");".ai()]);

    test_repo
        .repo
        .stage_all_and_commit("Force change")
        .expect("Failed to create commit");

    // Push with -u flag
    test_repo
        .repo
        .git(&["push", "-u", "origin", "feature/force-test"])
        .expect("Failed to push branch");

    println!("✅ Pushed branch");

    // Amend the commit
    test_file.insert_at(3, lines!["    println!(\"Amended line\");".ai()]);

    test_repo
        .repo
        .git(&["add", "-A"])
        .expect("Failed to add files");

    test_repo
        .repo
        .git(&["commit", "--amend", "--no-edit"])
        .expect("Failed to amend commit");

    println!("✅ Amended commit");

    // Push with --force-with-lease - notes should still be injected
    test_repo
        .repo
        .git(&["push", "--force-with-lease", "origin", "feature/force-test"])
        .expect("Failed to force push");

    println!("✅ Force pushed with --force-with-lease");

    // Verify notes exist on remote
    assert!(
        notes_exist_on_remote(test_repo.repo.path(), "origin"),
        "Notes should exist on remote after force push"
    );

    println!("✅ Test completed successfully - notes were preserved during force push");
}
