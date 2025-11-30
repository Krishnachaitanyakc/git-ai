use rand::seq::SliceRandom;
use rand::thread_rng;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

mod repos;
use git_ai::observability::wrapper_performance_targets::BenchmarkResult;
use repos::test_repo::TestRepo;

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case("chromium")]
    #[case("react")]
    #[case("node")]
    #[case("chakracore")]
    fn test_human_only_edits_then_commit(#[case] repo_name: &str) {
        let repos = get_performance_repos();
        let test_repo = repos
            .get(repo_name)
            .expect(&format!("{} repo should be available", repo_name));

        // Find random files for testing
        let random_files = find_random_files(test_repo).expect("Should find random files");

        // Select 3 random files (not large ones)
        let files_to_edit: Vec<String> =
            random_files.random_files.iter().take(3).cloned().collect();

        assert!(
            files_to_edit.len() >= 3,
            "Should have at least 3 random files to edit"
        );

        println!(
            "\n=== Testing {} - Editing {} files ===",
            repo_name,
            files_to_edit.len()
        );
        for (i, file) in files_to_edit.iter().enumerate() {
            println!("  {}. {}", i + 1, file);
        }

        // Create a sampler that runs 10 times
        let sampler = Sampler::new(10);

        // Sample the performance of human-only edits + commit
        let result = sampler.sample(test_repo, |repo| {
            // Append "# Human Line" to each file
            for file_path in &files_to_edit {
                let full_path = repo.path().join(file_path);

                let mut file = OpenOptions::new()
                    .append(true)
                    .open(&full_path)
                    .expect(&format!("Should be able to open file: {}", file_path));

                file.write_all(b"\n# Human Line\n")
                    .expect(&format!("Should be able to write to file: {}", file_path));
            }

            // Stage the files (regular git, no benchmark)
            for file_path in &files_to_edit {
                repo.git(&["add", file_path])
                    .expect(&format!("Should be able to stage file: {}", file_path));
            }

            // Benchmark the commit operation (where pre-commit hook runs)
            repo.benchmark_git(&["commit", "-m", "Human-only edits"])
                .expect("Commit should succeed")
        });

        // Print the results
        result.print_summary(&format!("Human-only edits + commit ({})", repo_name));
    }

    #[test]
    fn test_find_random_files() {
        let repos = get_performance_repos();
        let react_repo = repos.get("react").expect("React repo should be available");

        // Find random files for testing
        let random_files = find_random_files(react_repo).expect("Should find random files");

        println!("\n=== Random Files ===");
        for (i, file) in random_files.random_files.iter().enumerate() {
            println!("  {}. {}", i + 1, file);
        }

        println!("\n=== Large Files ===");
        for (i, file) in random_files.large_files.iter().enumerate() {
            println!("  {}. {}", i + 1, file);
        }

        // Verify we got files
        assert!(
            !random_files.random_files.is_empty(),
            "Should have random files"
        );
        assert!(
            !random_files.large_files.is_empty(),
            "Should have at least some large files"
        );
    }

    #[test]
    fn test_benchmark_git() {
        let repos = get_performance_repos();
        let react_repo = repos
            .get("chromium")
            .expect("React repo should be available");

        // Create a sampler that runs 5 times
        let sampler = Sampler::new(10);

        // Sample the performance of git log
        let result = sampler.sample(react_repo, |repo| {
            repo.benchmark_git(&["status"]).expect("log should succeed")
        });

        // Print the results
        result.print_summary("git log (100 commits)");
    }
}

const PERFORMANCE_REPOS: &[(&str, &str)] = &[
    ("chromium", "https://github.com/chromium/chromium.git"),
    ("react", "https://github.com/facebook/react.git"),
    ("node", "https://github.com/nodejs/node.git"),
    ("chakracore", "https://github.com/microsoft/ChakraCore.git"),
];

static PERFORMANCE_REPOS_MAP: OnceLock<HashMap<String, TestRepo>> = OnceLock::new();

fn clone_and_init_repos() -> HashMap<String, TestRepo> {
    // Determine the project root (where Cargo.toml is)
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let project_root = PathBuf::from(manifest_dir);
    let perf_repos_dir = project_root.join(".performance-repos");

    // Create .performance-repos directory if it doesn't exist
    if !perf_repos_dir.exists() {
        std::fs::create_dir_all(&perf_repos_dir)
            .expect("Failed to create .performance-repos directory");
    }

    let mut repos_map = HashMap::new();

    for (name, url) in PERFORMANCE_REPOS {
        let repo_path = perf_repos_dir.join(name);

        // Check if repository is already cloned
        if !(repo_path.exists() && repo_path.join(".git").exists()) {
            // Clone the repository with full history
            let output = Command::new("git")
                .args(&["clone", url, name])
                .current_dir(&perf_repos_dir)
                .output()
                .expect(&format!("Failed to clone repository: {}", name));

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                panic!("Failed to clone {}: {}", name, stderr);
            }
        }

        // Checkout the default branch (try main first, fallback to master)
        let checkout_main = Command::new("git")
            .args(&["checkout", "main"])
            .current_dir(&repo_path)
            .output();

        if checkout_main.is_err() || !checkout_main.unwrap().status.success() {
            // Try master if main doesn't exist
            let checkout_master = Command::new("git")
                .args(&["checkout", "master"])
                .current_dir(&repo_path)
                .output()
                .expect(&format!("Failed to checkout default branch for {}", name));

            if !checkout_master.status.success() {
                eprintln!("Warning: Could not checkout main or master for {}", name);
            }
        }

        // Create a test branch with timestamp to ensure clean state
        let timestamp_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards")
            .as_nanos();
        let branch_name = format!("test-cases/{}", timestamp_nanos);

        let create_branch = Command::new("git")
            .args(&["checkout", "-b", &branch_name])
            .current_dir(&repo_path)
            .output()
            .expect(&format!("Failed to create test branch for {}", name));

        if !create_branch.status.success() {
            let stderr = String::from_utf8_lossy(&create_branch.stderr);
            eprintln!(
                "Warning: Could not create test branch for {}: {}",
                name, stderr
            );
        }

        // Create TestRepo wrapper for the cloned repository
        let test_repo = TestRepo::new_at_path(&repo_path);
        repos_map.insert(name.to_string(), test_repo);
    }
    repos_map
}

/// Get the performance test repositories
/// This function ensures repositories are cloned and initialized only once
pub fn get_performance_repos() -> &'static HashMap<String, TestRepo> {
    PERFORMANCE_REPOS_MAP.get_or_init(clone_and_init_repos)
}

/// Result of finding random files in a repository
#[derive(Debug)]
pub struct RandomFiles {
    /// 10 random files from the repository
    pub random_files: Vec<String>,
    /// 2 random large files (5k-10k lines)
    pub large_files: Vec<String>,
}

/// Find random files in a repository for performance testing
///
/// Returns:
/// - 10 random files from the repository
/// - 2 random large files that are between 5k-10k lines
///
/// This helper is useful for performance testing various operations on different file sizes
pub fn find_random_files(test_repo: &TestRepo) -> Result<RandomFiles, String> {
    use git_ai::git::repository::find_repository_in_path;

    // Get the underlying Repository from the TestRepo path
    let repo = find_repository_in_path(test_repo.path().to_str().unwrap())
        .map_err(|e| format!("Failed to find repository: {:?}", e))?;

    // Get HEAD commit
    let head = repo
        .head()
        .map_err(|e| format!("Failed to get HEAD: {:?}", e))?;
    let head_commit = head
        .target()
        .map_err(|e| format!("Failed to get HEAD target: {:?}", e))?;

    // Use git ls-tree to get all files in the repository at HEAD
    let mut args = repo.global_args_for_exec();
    args.push("ls-tree".to_string());
    args.push("-r".to_string()); // Recursive
    args.push("--name-only".to_string());
    args.push(head_commit.clone());

    let output = Command::new(git_ai::config::Config::get().git_cmd())
        .args(&args)
        .output()
        .map_err(|e| format!("Failed to run git ls-tree: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "git ls-tree failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let all_files: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.is_empty())
        .map(|s| s.to_string())
        .collect();

    if all_files.is_empty() {
        return Err("No files found in repository".to_string());
    }

    // Select 10 random files
    let mut rng = thread_rng();
    let mut random_files: Vec<String> = all_files
        .choose_multiple(&mut rng, 10.min(all_files.len()))
        .cloned()
        .collect();

    // Find large files (5k-10k lines)
    let mut large_files: Vec<String> = Vec::new();

    // Shuffle to randomize the search order
    let mut shuffled_files = all_files.clone();
    shuffled_files.shuffle(&mut rng);

    for file_path in shuffled_files {
        if large_files.len() >= 2 {
            break;
        }

        // Read file content from HEAD
        let file_content = match repo.get_file_content(&file_path, &head_commit) {
            Ok(content) => content,
            Err(_) => continue, // Skip files that can't be read (binaries, etc.)
        };

        // Count lines
        let line_count = file_content.iter().filter(|&&b| b == b'\n').count();

        if line_count >= 5000 && line_count <= 10000 {
            large_files.push(file_path);
        }
    }

    // If we couldn't find 2 large files, fall back to the largest files we can find
    if large_files.len() < 2 {
        let mut file_sizes: Vec<(String, usize)> = Vec::new();

        // Sample a subset of files to check (to avoid checking all files in huge repos)
        let sample_size = 1000.min(all_files.len());
        let sample: Vec<String> = all_files
            .choose_multiple(&mut rng, sample_size)
            .cloned()
            .collect();

        for file_path in sample {
            if large_files.contains(&file_path) {
                continue;
            }

            if let Ok(content) = repo.get_file_content(&file_path, &head_commit) {
                let line_count = content.iter().filter(|&&b| b == b'\n').count();
                if line_count >= 1000 {
                    // Only consider reasonably large files
                    file_sizes.push((file_path, line_count));
                }
            }
        }

        // Sort by line count descending
        file_sizes.sort_by(|a, b| b.1.cmp(&a.1));

        // Take additional large files to reach 2 total
        for (file_path, _line_count) in file_sizes.iter().take(2 - large_files.len()) {
            large_files.push(file_path.clone());
        }
    }

    // Make sure random_files doesn't overlap with large_files
    random_files.retain(|f| !large_files.contains(f));

    // If we removed some, add more random files
    while random_files.len() < 10 && random_files.len() < all_files.len() {
        if let Some(file) = all_files
            .choose(&mut rng)
            .filter(|f| !random_files.contains(f) && !large_files.contains(f))
        {
            random_files.push(file.clone());
        } else {
            break;
        }
    }

    Ok(RandomFiles {
        random_files,
        large_files,
    })
}

/// Result of sampling a benchmark operation over multiple runs
#[derive(Debug, Clone)]
pub struct BenchmarkSampleResult {
    /// Number of runs performed
    pub num_runs: usize,
    /// Average benchmark result across all runs
    pub average: BenchmarkResult,
    /// Minimum benchmark result
    pub min: BenchmarkResult,
    /// Maximum benchmark result
    pub max: BenchmarkResult,
    /// All individual benchmark results
    pub results: Vec<BenchmarkResult>,
}

impl BenchmarkSampleResult {
    /// Print a formatted summary of the benchmark sample results
    pub fn print_summary(&self, operation_name: &str) {
        println!("\n=== Benchmark Summary: {} ===", operation_name);
        println!("  Runs:       {}", self.num_runs);
        println!(
            "  Average Total Duration:    {:?}",
            self.average.total_duration
        );
        println!(
            "  Average Git Duration:     {:?}",
            self.average.git_duration
        );
        println!(
            "  Average Pre-command:      {:?}",
            self.average.pre_command_duration
        );
        println!(
            "  Average Post-command:     {:?}",
            self.average.post_command_duration
        );
        println!("  Min Total Duration:       {:?}", self.min.total_duration);
        println!("  Max Total Duration:       {:?}", self.max.total_duration);

        // Calculate overhead statistics (where total > git)
        let overhead_results: Vec<_> = self
            .results
            .iter()
            .filter(|r| r.total_duration > r.git_duration)
            .collect();

        if !overhead_results.is_empty() {
            // Calculate average absolute overhead
            let total_overhead: Duration = overhead_results
                .iter()
                .map(|r| r.total_duration - r.git_duration)
                .sum();
            let avg_absolute_overhead = total_overhead / overhead_results.len() as u32;

            // Calculate average percentage overhead
            let total_percentage_overhead: f64 = overhead_results
                .iter()
                .map(|r| {
                    let overhead = r.total_duration.as_secs_f64() - r.git_duration.as_secs_f64();
                    let git_time = r.git_duration.as_secs_f64();
                    if git_time > 0.0 {
                        (overhead / git_time) * 100.0
                    } else {
                        0.0
                    }
                })
                .sum();
            let avg_percentage_overhead = total_percentage_overhead / overhead_results.len() as f64;

            println!(
                "  Overhead Cases:           {} (out of {})",
                overhead_results.len(),
                self.num_runs
            );
            println!("  Average Absolute Overhead: {:?}", avg_absolute_overhead);
            println!(
                "  Average % Overhead:       {:.2}%",
                avg_percentage_overhead
            );
        } else {
            println!("  Overhead Cases:           0 (out of {})", self.num_runs);
            println!("  Average Absolute Overhead: N/A (no overhead cases)");
            println!("  Average % Overhead:       N/A (no overhead cases)");
        }
    }
}

/// A sampler for measuring performance of operations on test repositories
pub struct Sampler {
    num_runs: usize,
}

impl Sampler {
    /// Create a new sampler that will run operations n times
    pub fn new(num_runs: usize) -> Self {
        assert!(num_runs > 0, "num_runs must be greater than 0");
        Self { num_runs }
    }

    /// Sample a benchmark operation over multiple runs
    ///
    /// # Arguments
    /// * `test_repo` - The test repository to pass to the operation
    /// * `operation` - A closure that takes a &TestRepo and returns a BenchmarkResult
    ///
    /// # Returns
    /// A `BenchmarkSampleResult` containing averaged statistics about the benchmark results
    ///
    /// # Example
    /// ```ignore
    /// let sampler = Sampler::new(5);
    /// let result = sampler.sample(test_repo, |repo| {
    ///     repo.benchmark_git(&["log", "--oneline", "-n", "100"])
    ///         .expect("log should succeed")
    /// });
    /// result.print_summary("git log (100 commits)");
    /// ```
    pub fn sample<F>(&self, test_repo: &TestRepo, operation: F) -> BenchmarkSampleResult
    where
        F: Fn(&TestRepo) -> BenchmarkResult,
    {
        let mut results = Vec::with_capacity(self.num_runs);

        for _i in 0..self.num_runs {
            let benchmark_result = operation(test_repo);
            results.push(benchmark_result);
        }

        // Calculate averages for each duration field
        let total_total: Duration = results.iter().map(|r| r.total_duration).sum();
        let total_git: Duration = results.iter().map(|r| r.git_duration).sum();
        let total_pre: Duration = results.iter().map(|r| r.pre_command_duration).sum();
        let total_post: Duration = results.iter().map(|r| r.post_command_duration).sum();

        let average = BenchmarkResult {
            total_duration: total_total / self.num_runs as u32,
            git_duration: total_git / self.num_runs as u32,
            pre_command_duration: total_pre / self.num_runs as u32,
            post_command_duration: total_post / self.num_runs as u32,
        };

        // Find min and max based on total_duration
        let min = results
            .iter()
            .min_by_key(|r| r.total_duration)
            .unwrap()
            .clone();
        let max = results
            .iter()
            .max_by_key(|r| r.total_duration)
            .unwrap()
            .clone();

        BenchmarkSampleResult {
            num_runs: self.num_runs,
            average,
            min,
            max,
            results,
        }
    }
}
