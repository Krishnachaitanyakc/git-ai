use std::collections::HashSet;

use crate::authorship::authorship_log_serialization::AuthorshipLog;
use crate::error::GitAiError;
use crate::git::repository::{Repository, exec_git, exec_git_stdin};

/// Get a HashSet of all files that have AI attributions across all commits
///
/// Efficiently loads all notes and extracts unique file paths without keeping
/// full attestations in memory
pub async fn load_all_ai_touched_files(repo: &Repository) -> Result<HashSet<String>, GitAiError> {
    let global_args = repo.global_args_for_exec();

    // Run in blocking context since we're doing I/O
    smol::unblock(move || load_all_ai_touched_files_sync(&global_args)).await
}

fn load_all_ai_touched_files_sync(global_args: &[String]) -> Result<HashSet<String>, GitAiError> {
    // Step 1: Get all blob entries from refs/notes/ai using ls-tree
    let blob_shas = get_note_blob_shas(global_args)?;

    if blob_shas.is_empty() {
        return Ok(HashSet::new());
    }

    // Step 2: Use cat-file --batch to read all blobs efficiently
    let blob_contents = batch_read_blobs(global_args, &blob_shas)?;

    // Step 3: Extract file paths from all blob contents
    let mut all_files = HashSet::new();
    for content in blob_contents {
        extract_file_paths_from_note(&content, &mut all_files);
    }

    Ok(all_files)
}

/// Get all blob SHAs from refs/notes/ai tree
fn get_note_blob_shas(global_args: &[String]) -> Result<Vec<String>, GitAiError> {
    let mut args = global_args.to_vec();
    args.push("ls-tree".to_string());
    args.push("-r".to_string());
    args.push("refs/notes/ai".to_string());

    let output = match exec_git(&args) {
        Ok(output) => output,
        Err(GitAiError::GitCliError {
            code: Some(128), ..
        }) => {
            // refs/notes/ai doesn't exist - no notes yet
            return Ok(Vec::new());
        }
        Err(e) => return Err(e),
    };

    let stdout = String::from_utf8(output.stdout)?;

    // Parse ls-tree output: "<mode> <type> <object>\t<path>"
    let mut blob_shas = Vec::new();
    for line in stdout.lines() {
        if line.is_empty() {
            continue;
        }
        // Split on whitespace to get mode, type, object
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 3 {
            // parts[2] is the object SHA (blob)
            // The path comes after a tab, but we don't need it
            let sha = parts[2].split('\t').next().unwrap_or(parts[2]);
            blob_shas.push(sha.to_string());
        }
    }

    Ok(blob_shas)
}

/// Read multiple blobs efficiently using cat-file --batch
fn batch_read_blobs(
    global_args: &[String],
    blob_shas: &[String],
) -> Result<Vec<String>, GitAiError> {
    if blob_shas.is_empty() {
        return Ok(Vec::new());
    }

    let mut args = global_args.to_vec();
    args.push("cat-file".to_string());
    args.push("--batch".to_string());

    // Prepare stdin: one SHA per line
    let stdin_data = blob_shas.join("\n") + "\n";

    let output = exec_git_stdin(&args, stdin_data.as_bytes())?;

    // Parse batch output
    // Format for each object:
    // <sha> <type> <size>\n
    // <content>\n
    parse_cat_file_batch_output(&output.stdout)
}

/// Parse the output of git cat-file --batch
///
/// Format:
/// <sha> <type> <size>\n
/// <content bytes>\n
/// (repeat for each object)
fn parse_cat_file_batch_output(data: &[u8]) -> Result<Vec<String>, GitAiError> {
    let mut results = Vec::new();
    let mut pos = 0;

    while pos < data.len() {
        // Find the header line ending with \n
        let header_end = match data[pos..].iter().position(|&b| b == b'\n') {
            Some(idx) => pos + idx,
            None => break,
        };

        let header = std::str::from_utf8(&data[pos..header_end])
            .map_err(|e| GitAiError::Generic(format!("Invalid UTF-8 in header: {}", e)))?;

        // Parse header: "<sha> <type> <size>" or "<sha> missing"
        let parts: Vec<&str> = header.split_whitespace().collect();
        if parts.len() < 2 {
            pos = header_end + 1;
            continue;
        }

        if parts[1] == "missing" {
            // Object doesn't exist, skip
            pos = header_end + 1;
            continue;
        }

        if parts.len() < 3 {
            pos = header_end + 1;
            continue;
        }

        let size: usize = parts[2]
            .parse()
            .map_err(|e| GitAiError::Generic(format!("Invalid size in cat-file output: {}", e)))?;

        // Content starts after the header newline
        let content_start = header_end + 1;
        let content_end = content_start + size;

        if content_end > data.len() {
            break;
        }

        // Try to parse content as UTF-8
        if let Ok(content) = std::str::from_utf8(&data[content_start..content_end]) {
            results.push(content.to_string());
        }

        // Move past content and the trailing newline
        pos = content_end + 1;
    }

    Ok(results)
}

/// Extract file paths from a note blob content
fn extract_file_paths_from_note(content: &str, files: &mut HashSet<String>) {
    // Find the divider and slice before it, then add minimal metadata to make it parseable
    if let Some(divider_pos) = content.find("\n---\n") {
        let attestation_section = &content[..divider_pos];
        // Create a complete parseable format with empty metadata
        let parseable = format!(
            "{}\n---\n{{\"schema_version\":\"authorship/3.0.0\",\"base_commit_sha\":\"\",\"prompts\":{{}}}}",
            attestation_section
        );

        if let Ok(log) = AuthorshipLog::deserialize_from_string(&parseable) {
            for attestation in log.attestations {
                files.insert(attestation.file_path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{find_repository_in_path, sync_authorship::fetch_authorship_notes};
    use std::time::Instant;

    #[test]
    fn test_load_ai_touched_files() {
        smol::block_on(async {
            let repo = find_repository_in_path(".").unwrap();

            fetch_authorship_notes(&repo, "origin").unwrap();

            let start = Instant::now();
            let files = load_all_ai_touched_files(&repo).await.unwrap();
            let elapsed = start.elapsed();

            println!(
                "Found {} unique AI-touched files in {:?}",
                files.len(),
                elapsed
            );

            // Show first 10 files
            let mut sorted_files: Vec<_> = files.iter().collect();
            sorted_files.sort();
            for file in sorted_files.iter().take(10) {
                println!("  {}", file);
            }
        });
    }
}
