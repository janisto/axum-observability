//! Repository policy for automated GitHub REST callers.

use std::{fs, path::Path};

const AUTOMATED_EXTENSIONS: &[&str] = &[
    "bash", "cjs", "go", "js", "json", "mjs", "py", "rs", "sh", "toml", "ts", "yaml", "yml", "zsh",
];
const SKIPPED_DIRECTORIES: &[&str] = &[
    ".git",
    ".venv",
    "artifacts",
    "coverage",
    "dist",
    "mutants",
    "node_modules",
    "target",
];

fn client_aliases(content: &str) -> Vec<String> {
    content
        .lines()
        .filter_map(|line| {
            let (left, right) = line.split_once('=')?;
            if !["Octokit", "GitHub", "Github", "octokit", "github"]
                .iter()
                .any(|marker| right.contains(marker))
            {
                return None;
            }
            left.split_whitespace()
                .last()
                .map(|alias| alias.trim_end_matches(':').to_owned())
        })
        .collect()
}

fn is_caller(line: &str, aliases: &[String]) -> bool {
    let normalized_whitespace = line.split_whitespace().collect::<Vec<_>>().join(" ");
    normalized_whitespace.contains("gh api")
        || line.contains("github.rest")
        || line.contains("github.request(")
        || line.contains("github.paginate(")
        || line.contains("octokit.rest")
        || line.contains("octokit.request(")
        || line.contains("octokit.paginate(")
        || line.contains("https://api.github.com")
        || line.contains("http://api.github.com")
        || aliases.iter().any(|alias| {
            [".rest", ".request(", ".paginate("]
                .iter()
                .any(|method| line.contains(&format!("{alias}{method}")))
        })
}

fn has_locked_header(unit: &str) -> bool {
    let compact = unit
        .chars()
        .filter(|character| !character.is_whitespace() && *character != '"' && *character != '\'')
        .collect::<String>();
    unit.matches("X-GitHub-Api-Version").count() == 1
        && (compact.contains("X-GitHub-Api-Version:2026-03-10")
            || compact.contains("X-GitHub-Api-Version=2026-03-10"))
}

fn is_automated_path(path: &str) -> bool {
    let path = Path::new(path);
    if path.file_name().and_then(|name| name.to_str()) == Some("github_api_policy.rs") {
        return false;
    }
    if path.extension().and_then(|extension| extension.to_str()) == Some("md") {
        return false;
    }
    path.file_name().and_then(|name| name.to_str()) == Some("Justfile")
        || path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| AUTOMATED_EXTENSIONS.contains(&extension))
}

fn policy_violations(files: &[(String, String)]) -> Vec<String> {
    let mut violations = Vec::new();
    for (path, content) in files {
        if !is_automated_path(path) {
            continue;
        }
        let lines = content.lines().collect::<Vec<_>>();
        let aliases = client_aliases(content);
        for (index, line) in lines.iter().enumerate() {
            if !is_caller(line, &aliases) {
                continue;
            }
            let limit = lines.len().min(index + 12);
            let mut end = index + 1;
            while end < limit && !is_caller(lines[end], &aliases) {
                end += 1;
            }
            if !has_locked_header(&lines[index..end].join("\n")) {
                violations.push(format!("{path}:{}", index + 1));
            }
        }
    }
    violations.sort();
    violations
}

fn collect_repository_files(directory: &Path, files: &mut Vec<(String, String)>) {
    for entry in fs::read_dir(directory).expect("read repository directory") {
        let entry = entry.expect("read repository entry");
        let path = entry.path();
        if path.is_dir() {
            if !entry
                .file_name()
                .to_str()
                .is_some_and(|name| SKIPPED_DIRECTORIES.contains(&name))
            {
                collect_repository_files(&path, files);
            }
            continue;
        }
        let relative = path
            .strip_prefix(".")
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();
        if is_automated_path(&relative) {
            files.push((
                relative,
                fs::read_to_string(path).expect("read repository policy file"),
            ));
        }
    }
}

#[test]
fn zero_automated_callers_pass_and_human_documentation_is_ignored() {
    let files = vec![(
        "README.md".to_owned(),
        "Use `gh api` with the locally installed CLI.".to_owned(),
    )];
    assert!(policy_violations(&files).is_empty());
}

#[test]
fn exact_locked_header_passes() {
    let files = vec![(
        "workflow.yml".to_owned(),
        "github.request(\"GET /repo\", {\nheaders: {\"X-GitHub-Api-Version\": \"2026-03-10\"}\n})"
            .to_owned(),
    )];
    assert!(policy_violations(&files).is_empty());
}

#[test]
fn missing_dynamic_and_different_headers_fail() {
    for content in [
        "github.request(\"GET /repo\")",
        "github.request(\"GET /repo\", header=VERSION)",
        "github.request(\"GET /repo\", header=\"X-GitHub-Api-Version: 2022-11-28\")",
    ] {
        let files = vec![("client.rs".to_owned(), content.to_owned())];
        assert_eq!(policy_violations(&files), ["client.rs:1"]);
    }
}

#[test]
fn one_pinned_caller_does_not_mask_an_unpinned_caller() {
    let files = vec![(
        "client.rs".to_owned(),
        "github.request(\"GET /one\", header=\"X-GitHub-Api-Version: 2026-03-10\")\ngithub.request(\"GET /two\")"
            .to_owned(),
    )];
    assert_eq!(policy_violations(&files), ["client.rs:2"]);
}

#[test]
fn conflicting_versions_in_one_caller_fail() {
    let files = vec![(
        "client.rs".to_owned(),
        "github.request(\"GET /one\", header=\"X-GitHub-Api-Version: 2026-03-10\")\nheader=\"X-GitHub-Api-Version: 2022-11-28\""
            .to_owned(),
    )];
    assert_eq!(policy_violations(&files), ["client.rs:1"]);
}

#[test]
fn aliased_octokit_caller_is_detected() {
    let files = vec![(
        "client.rs".to_owned(),
        "const client = new Octokit()\nclient.request(\"GET /repo\")".to_owned(),
    )];
    assert_eq!(policy_violations(&files), ["client.rs:2"]);
}

#[test]
fn repository_has_no_unpinned_automated_github_rest_caller() {
    let mut files = Vec::new();
    collect_repository_files(Path::new("."), &mut files);
    let violations = policy_violations(&files);
    assert!(
        violations.is_empty(),
        "unpinned automated GitHub REST callers: {violations:?}"
    );
}
