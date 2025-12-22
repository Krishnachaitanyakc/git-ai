use crate::commands::git_handlers::CommandHooksContext;
use crate::commands::upgrade;
use crate::git::cli_parser::{ParsedGitInvocation, is_dry_run};
use crate::git::repository::{Repository, exec_git, find_repository};
use crate::git::sync_authorship::{fetch_authorship_notes, fetch_remote_from_args};
use crate::utils::debug_log;

pub fn fetch_pull_pre_command_hook(
    parsed_args: &ParsedGitInvocation,
    repository: &Repository,
) -> Option<std::thread::JoinHandle<()>> {
    upgrade::maybe_schedule_background_update_check();

    // Early return for dry-run
    if is_dry_run(&parsed_args.command_args) {
        return None;
    }

    crate::observability::spawn_background_flush();

    // Extract the remote name
    let remote = match fetch_remote_from_args(repository, parsed_args) {
        Ok(remote) => remote,
        Err(_) => {
            debug_log("failed to extract remote for authorship fetch; skipping");
            return None;
        }
    };

    // Clone what we need for the background thread
    let global_args = repository.global_args_for_exec();

    // Spawn background thread to fetch authorship notes in parallel with main fetch
    Some(std::thread::spawn(move || {
        debug_log(&format!(
            "started fetching authorship notes from remote: {}",
            remote
        ));
        // Recreate repository in the background thread
        if let Ok(repo) = find_repository(&global_args) {
            if let Err(e) = fetch_authorship_notes(&repo, &remote) {
                debug_log(&format!("authorship fetch failed: {}", e));
            }
        } else {
            debug_log("failed to open repository for authorship fetch");
        }
    }))
}

pub fn fetch_pull_post_command_hook(
    _repository: &Repository,
    _parsed_args: &ParsedGitInvocation,
    _exit_status: std::process::ExitStatus,
    command_hooks_context: &mut CommandHooksContext,
) {
    // Always wait for the authorship fetch thread to complete if it was started,
    // regardless of whether the main fetch/pull succeeded or failed.
    // This ensures proper cleanup of the background thread.
    if let Some(handle) = command_hooks_context.fetch_authorship_handle.take() {
        let _ = handle.join();
    }
}

pub fn pull_post_command_hook(
    repository: &mut Repository,
    _parsed_args: &ParsedGitInvocation,
    exit_status: std::process::ExitStatus,
    command_hooks_context: &mut CommandHooksContext,
) {
    // Wait for authorship fetch thread
    if let Some(handle) = command_hooks_context.fetch_authorship_handle.take() {
        let _ = handle.join();
    }

    if !exit_status.success() {
        return;
    }

    // Get old HEAD from pre-command capture
    let old_head = match &repository.pre_command_base_commit {
        Some(sha) => sha.clone(),
        None => return,
    };

    // Get new HEAD
    let new_head = match repository.head().ok().and_then(|h| h.target().ok()) {
        Some(sha) => sha,
        None => return,
    };

    if old_head == new_head {
        return;
    }

    // Check reflog for fast-forward indicator, verifying the SHA matches our new HEAD
    if !was_fast_forward_pull(repository, &new_head) {
        return;
    }

    debug_log(&format!(
        "Fast-forward detected: {} -> {}",
        old_head, new_head
    ));
    let _ = repository.storage.rename_working_log(&old_head, &new_head);
}

/// Check if the most recent reflog entry indicates a fast-forward pull operation.
/// Uses format "%H %gs" to get both the commit SHA and the reflog subject.
/// Verifies:
/// 1. The reflog SHA matches the expected new HEAD (confirms we have the right entry)
/// 2. The subject starts with "pull" (confirms it was a pull operation)
/// 3. The subject ends with ": Fast-forward" (confirms it was a fast-forward)
fn was_fast_forward_pull(repository: &Repository, expected_new_head: &str) -> bool {
    let mut args = repository.global_args_for_exec();
    args.extend(
        ["reflog", "-1", "--format=%H %gs"]
            .iter()
            .map(|s| s.to_string()),
    );

    match exec_git(&args) {
        Ok(output) => {
            let output_str = String::from_utf8_lossy(&output.stdout);
            let output_str = output_str.trim();

            // Format: "<sha> <subject>"
            // Example: "1f9a5dc45612afcbef17e9d07441d9b57c7bb5d0 pull: Fast-forward"
            let Some((sha, subject)) = output_str.split_once(' ') else {
                return false;
            };

            // Verify the SHA matches our expected new HEAD
            if sha != expected_new_head {
                debug_log(&format!(
                    "Reflog SHA {} doesn't match expected HEAD {}",
                    sha, expected_new_head
                ));
                return false;
            }

            // Must be a pull command that resulted in fast-forward
            subject.starts_with("pull") && subject.ends_with(": Fast-forward")
        }
        Err(_) => false,
    }
}
