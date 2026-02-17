// Integration test utilities and fixture management for Homer.

use std::path::Path;
use std::process::Command;

/// A test fixture with a temporary git repository.
#[derive(Debug)]
pub struct TestRepo {
    pub dir: tempfile::TempDir,
}

impl TestRepo {
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Create a minimal Rust project with git history.
    pub fn minimal_rust() -> Self {
        let dir = tempfile::tempdir().expect("create tempdir");
        let root = dir.path();

        // Initialize git repo
        git(root, &["init"]);
        git(root, &["config", "user.email", "test@homer.dev"]);
        git(root, &["config", "user.name", "Test"]);

        // Commit 1: Initial structure
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"test-project\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"1.0\"\ntokio = { version = \"1\", features = [\"full\"] }\n",
        ).unwrap();
        std::fs::write(
            root.join("src/main.rs"),
            "fn main() {\n    greet();\n}\n\nfn greet() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("README.md"),
            "# Test Project\n\n## Overview\n\nA test project using [main](src/main.rs).\n",
        )
        .unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-m", "Initial commit"]);

        // Commit 2: Add helper module
        std::fs::write(
            root.join("src/helpers.rs"),
            "pub fn format_name(name: &str) -> String {\n    format!(\"Hello, {name}!\")\n}\n",
        )
        .unwrap();
        write_line(
            root,
            "src/main.rs",
            "mod helpers;\n\nfn main() {\n    greet();\n    println!(\"{}\", helpers::format_name(\"world\"));\n}\n\nfn greet() {\n    println!(\"hello\");\n}\n",
        );
        git(root, &["add", "."]);
        git(root, &["commit", "-m", "Add helpers module"]);

        // Commit 3: Add tests
        std::fs::create_dir_all(root.join("tests")).unwrap();
        std::fs::write(
            root.join("tests/test_lib.rs"),
            "#[test]\nfn test_add() {\n    assert_eq!(test_project::add(2, 3), 5);\n}\n",
        )
        .unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-m", "Add tests"]);

        // Commit 4: Modify lib.rs
        write_line(
            root,
            "src/lib.rs",
            "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\npub fn multiply(a: i32, b: i32) -> i32 {\n    a * b\n}\n",
        );
        git(root, &["add", "."]);
        git(root, &["commit", "-m", "Add multiply function"]);

        // Commit 5: Update main
        write_line(
            root,
            "src/main.rs",
            "mod helpers;\n\nfn main() {\n    greet();\n    let result = test_project::multiply(3, 4);\n    println!(\"3 * 4 = {result}\");\n    println!(\"{}\", helpers::format_name(\"world\"));\n}\n\nfn greet() {\n    println!(\"hello\");\n}\n",
        );
        git(root, &["add", "."]);
        git(root, &["commit", "-m", "Use multiply in main"]);

        Self { dir }
    }

    /// Create a multi-language project (Rust + Python + TypeScript).
    pub fn multi_lang() -> Self {
        let dir = tempfile::tempdir().expect("create tempdir");
        let root = dir.path();

        git(root, &["init"]);
        git(root, &["config", "user.email", "test@homer.dev"]);
        git(root, &["config", "user.name", "Test"]);

        // Rust
        std::fs::create_dir_all(root.join("rust-svc/src")).unwrap();
        std::fs::write(
            root.join("rust-svc/Cargo.toml"),
            "[package]\nname = \"rust-svc\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("rust-svc/src/main.rs"),
            "fn main() {\n    println!(\"rust service\");\n}\n",
        )
        .unwrap();

        // Python
        std::fs::create_dir_all(root.join("py-lib")).unwrap();
        std::fs::write(
            root.join("py-lib/pyproject.toml"),
            "[project]\nname = \"py-lib\"\nversion = \"0.1.0\"\ndependencies = [\"requests>=2.28\"]\n",
        ).unwrap();
        std::fs::write(
            root.join("py-lib/main.py"),
            "import requests\n\ndef fetch_data(url: str) -> dict:\n    response = requests.get(url)\n    return response.json()\n\ndef process(data: dict) -> str:\n    return str(data)\n",
        ).unwrap();

        // TypeScript
        std::fs::create_dir_all(root.join("ts-app/src")).unwrap();
        std::fs::write(
            root.join("ts-app/package.json"),
            "{\"name\": \"ts-app\", \"version\": \"1.0.0\", \"dependencies\": {\"express\": \"^4.18\"}, \"scripts\": {\"build\": \"tsc\", \"test\": \"jest\"}}",
        ).unwrap();
        std::fs::write(
            root.join("ts-app/src/index.ts"),
            "import express from 'express';\n\nconst app = express();\n\nfunction greet(name: string): string {\n    return `Hello, ${name}!`;\n}\n\napp.get('/', (req, res) => {\n    res.send(greet('world'));\n});\n\nexport { greet };\n",
        ).unwrap();

        std::fs::write(root.join("README.md"), "# Multi-Language Project\n\n## Components\n\n- [Rust service](rust-svc/src/main.rs)\n- [Python library](py-lib/main.py)\n- [TypeScript app](ts-app/src/index.ts)\n").unwrap();

        git(root, &["add", "."]);
        git(root, &["commit", "-m", "Initial multi-lang project"]);

        // Commit 2: changes across languages
        write_line(
            root,
            "rust-svc/src/main.rs",
            "fn main() {\n    let msg = build_message();\n    println!(\"{msg}\");\n}\n\nfn build_message() -> String {\n    \"rust service v2\".to_string()\n}\n",
        );
        write_line(
            root,
            "py-lib/main.py",
            "import requests\n\ndef fetch_data(url: str) -> dict:\n    response = requests.get(url)\n    return response.json()\n\ndef process(data: dict) -> str:\n    return str(data)\n\ndef validate(data: dict) -> bool:\n    return 'status' in data\n",
        );
        git(root, &["add", "."]);
        git(root, &["commit", "-m", "Add build_message and validate"]);

        Self { dir }
    }

    /// Create a documentation-heavy project (README + ADR + doc comments).
    pub fn documented() -> Self {
        let dir = tempfile::tempdir().expect("create tempdir");
        let root = dir.path();

        git(root, &["init"]);
        git(root, &["config", "user.email", "test@homer.dev"]);
        git(root, &["config", "user.name", "Test"]);

        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("docs/adr")).unwrap();

        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"documented\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"1.0\"\n",
        ).unwrap();

        std::fs::write(
            root.join("src/lib.rs"),
            "/// The main processing pipeline.\n///\n/// Takes input data and transforms it through several stages.\npub fn process(input: &str) -> String {\n    validate(input);\n    transform(input)\n}\n\n/// Validates input data.\nfn validate(input: &str) {\n    assert!(!input.is_empty());\n}\n\n/// Transforms input by converting to uppercase.\nfn transform(input: &str) -> String {\n    input.to_uppercase()\n}\n",
        ).unwrap();

        std::fs::write(
            root.join("README.md"),
            "# Documented Project\n\n## Overview\n\nThis project demonstrates documentation patterns.\nSee [the main library](src/lib.rs) for the processing pipeline.\n\n## Architecture\n\nSee [ADR-001](docs/adr/001-processing-pipeline.md) for the design.\n\n## Getting Started\n\n```bash\ncargo build\ncargo test\n```\n",
        ).unwrap();

        std::fs::write(
            root.join("docs/adr/001-processing-pipeline.md"),
            "# ADR-001: Processing Pipeline Design\n\n## Status\n\nAccepted\n\n## Context\n\nWe need a processing pipeline that validates and transforms input.\n\n## Decision\n\nWe will use a simple two-stage pipeline: validate then transform.\nSee [lib.rs](../../src/lib.rs) for the implementation.\n\n## Consequences\n\n- Simple and easy to understand\n- Can extend with more stages later\n",
        ).unwrap();

        std::fs::write(root.join("CONTRIBUTING.md"), "# Contributing\n\n## Code Style\n\nRun `cargo fmt` and `cargo clippy` before submitting.\n\n## Testing\n\nAll PRs must include tests. Run `cargo test` to verify.\n").unwrap();

        git(root, &["add", "."]);
        git(root, &["commit", "-m", "Initial documented project"]);

        // Commit 2: update docs
        write_line(
            root,
            "README.md",
            "# Documented Project\n\n## Overview\n\nThis project demonstrates documentation patterns.\nSee [the main library](src/lib.rs) for the processing pipeline.\n\n## Architecture\n\nSee [ADR-001](docs/adr/001-processing-pipeline.md) for the design.\n\n## Getting Started\n\n```bash\ncargo build\ncargo test\n```\n\n## API\n\n- `process(input)` — main entry point\n",
        );
        git(root, &["add", "."]);
        git(root, &["commit", "-m", "Update README with API docs"]);

        Self { dir }
    }
}

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_DATE", "2025-01-15T10:00:00+00:00")
        .env("GIT_COMMITTER_DATE", "2025-01-15T10:00:00+00:00")
        .output()
        .unwrap_or_else(|e| panic!("git {}: {e}", args.join(" ")));
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!("git {} failed: {stderr}", args.join(" "));
    }
}

fn write_line(root: &Path, rel: &str, content: &str) {
    std::fs::write(root.join(rel), content).unwrap();
}

/// Run the full Homer pipeline on a repo path and return the result.
pub async fn run_pipeline(
    repo_path: &Path,
) -> homer_core::error::Result<homer_core::pipeline::PipelineResult> {
    let store = homer_core::store::sqlite::SqliteStore::in_memory().unwrap();
    let config = homer_core::config::HomerConfig::default();
    let pipeline = homer_core::pipeline::HomerPipeline::new(repo_path);
    pipeline.run(&store, &config).await
}

/// Run the full Homer pipeline and return both result and store for inspection.
pub async fn run_pipeline_with_store(
    repo_path: &Path,
) -> (
    homer_core::pipeline::PipelineResult,
    homer_core::store::sqlite::SqliteStore,
) {
    let store = homer_core::store::sqlite::SqliteStore::in_memory().unwrap();
    let config = homer_core::config::HomerConfig::default();
    let pipeline = homer_core::pipeline::HomerPipeline::new(repo_path);
    let result = pipeline.run(&store, &config).await.unwrap();
    (result, store)
}
