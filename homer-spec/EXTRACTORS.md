# Homer Extractors

> Git history, GitHub API, structure detection, graph extraction, document mining, and prompt extraction.

**Parent**: [README.md](README.md)  
**Related**: [ARCHITECTURE.md](ARCHITECTURE.md) · [STORE.md](STORE.md) · [GRAPH_ENGINE.md](GRAPH_ENGINE.md)

---

## Extractor Trait

All extractors implement a common trait:

```rust
#[async_trait]
pub trait Extractor: Send + Sync {
    fn name(&self) -> &str;
    
    /// Check if this extractor has new data to process
    async fn has_work(&self, store: &dyn HomerStore) -> Result<bool>;
    
    /// Run extraction, writing results to the store
    async fn extract(&self, store: &dyn HomerStore, config: &ExtractConfig) -> Result<ExtractStats>;
}

pub struct ExtractStats {
    pub nodes_created: u64,
    pub nodes_updated: u64,
    pub edges_created: u64,
    pub duration: Duration,
    pub errors: Vec<(String, HomerError)>,
}
```

---

## Git History Extractor

**Crate dependency**: `gix` (pure-Rust git implementation)

### What It Extracts

For each commit since the last checkpoint:

| Data | Node/Edge | Details |
|------|-----------|---------|
| Commit metadata | `Node(Commit)` | SHA, message, author, committer, timestamp, parent SHAs |
| Author | `Node(Contributor)` | Name, email (deduplicated by email) |
| Modified files | `Hyperedge(Modifies)` | Commit → {files}, with diff stats per file |
| Tags/releases | `Node(Release)` | Tag name, annotated message, target SHA |
| Release contents | `Hyperedge(Includes)` | Release → {commits between this and previous release} |
| Authorship | `Hyperedge(Authored)` | Contributor → {commits} |

### Diff Processing

For each commit's diff, extract per-file:

```rust
pub struct FileDiffStats {
    pub path: PathBuf,
    pub old_path: Option<PathBuf>,  // For renames
    pub status: DiffStatus,         // Added, Modified, Deleted, Renamed
    pub lines_added: u32,
    pub lines_deleted: u32,
    /// The actual diff hunks (stored in metadata, used by semantic analyzer)
    pub hunks: Vec<DiffHunk>,
}
```

**Renames**: When git detects a rename, Homer creates a new node for the new path and records the rename in metadata. The old node is marked stale but retained for historical analysis.

### Incrementality

```
Checkpoint: git_last_sha = "abc123"

On update:
1. repo.revwalk() from HEAD back to checkpoint SHA
2. Process each commit in topological order (oldest first)
3. After all commits processed: set checkpoint to HEAD SHA
```

**Edge case — force push / rebased history**: If the checkpoint SHA is not an ancestor of HEAD, Homer detects this and falls back to full re-extraction with a warning.

### Performance Notes

- Use `gix` diff options with binary check skipping for speed
- Batch commit processing: load 100 commits, process diffs, write to store in single transaction
- For initial extraction of very large repos (100K+ commits), provide progress reporting via callback

---

## GitHub API Extractor

**Crate dependencies**: `reqwest` (HTTP client)

### What It Extracts

| Data | Node/Edge | API Endpoint |
|------|-----------|-------------|
| Pull requests | `Node(PullRequest)` | `GET /repos/{owner}/{repo}/pulls` |
| PR reviews | `Hyperedge(Reviewed)` | `GET /repos/{owner}/{repo}/pulls/{n}/reviews` |
| PR → commit links | `Hyperedge(Modifies)` cross-ref | PR metadata contains merge commit SHA |
| Issues | `Node(Issue)` | `GET /repos/{owner}/{repo}/issues` |
| Issue → PR links | `Hyperedge(Resolves)` | Cross-reference parsing ("fixes #N", "closes #N") |
| PR/Issue comments | Stored in node metadata | `GET .../comments` |
| Labels, milestones | Stored in node metadata | Included in PR/issue response |

### Cross-Reference Resolution

Homer parses PR descriptions, commit messages, and issue bodies for cross-references:

```rust
/// Patterns that link PRs to issues
const CLOSE_PATTERNS: &[&str] = &[
    "close", "closes", "closed",
    "fix", "fixes", "fixed",
    "resolve", "resolves", "resolved",
];

/// Extract issue numbers from text
fn extract_issue_refs(text: &str, repo_owner: &str, repo_name: &str) -> Vec<IssueRef> {
    // Match: #123, GH-123, owner/repo#123, full URL
    // ...
}
```

### Rate Limiting

GitHub API has rate limits (5000 requests/hour for authenticated users). Homer must:

1. Read rate limit headers from responses (`X-RateLimit-Remaining`, `X-RateLimit-Reset`)
2. Exponential backoff when approaching limit
3. For large repos: paginate results (100 per page max)
4. Estimate total API calls needed and warn user if it will take long

### Incrementality

```
Checkpoint: github_last_pr = 142, github_last_issue = 87

On update:
1. Fetch PRs where number > 142 (sorted ascending)
2. Fetch issues where number > 87 (sorted ascending)
3. For updated PRs (state changes): re-fetch and update node
4. Update checkpoints to max numbers seen
```

### Configuration

```toml
[extraction.github]
# How to detect the remote
remote = "origin"                    # Git remote to read owner/repo from
# Or specify explicitly:
# owner = "rand"
# repo = "homer"

token_env = "GITHUB_TOKEN"          # Environment variable for auth token
max_pr_history = 500                # Max PRs to fetch on initial run
max_issue_history = 1000            # Max issues to fetch on initial run
include_comments = true             # Fetch PR/issue comments
include_reviews = true              # Fetch PR reviews
```

### Forge Abstraction

While Homer starts with GitHub, the extractor should be designed to support GitLab and others:

```rust
pub trait ForgeExtractor: Extractor {
    fn forge_type(&self) -> ForgeType;  // GitHub, GitLab, Bitbucket
    fn detect(repo_path: &Path) -> Option<ForgeConfig>;  // Auto-detect from remotes
}
```

---

## GitLab API Extractor

**Crate dependencies**: `reqwest` (HTTP client)

### What It Extracts

| Data | Node/Edge | API Endpoint |
|------|-----------|-------------|
| Merge requests | `Node(PullRequest)` | `GET /projects/{id}/merge_requests` |
| MR approvals | `Hyperedge(Reviewed)` | `GET /projects/{id}/merge_requests/{iid}/approvals` |
| MR → commit links | `Hyperedge(Modifies)` cross-ref | MR metadata contains merge commit SHA |
| Issues | `Node(Issue)` | `GET /projects/{id}/issues` |
| Issue → MR links | `Hyperedge(Resolves)` | Cross-reference parsing ("closes #N", "fixes #N") |
| Labels | Stored in node metadata | Included in issue response |

### Remote Detection

Homer detects GitLab remotes from the repository's push URL. Both SSH (`git@gitlab.example.com:owner/repo.git`) and HTTPS (`https://gitlab.example.com/owner/repo.git`) formats are supported, including self-hosted instances (any host containing "gitlab").

### Node Mapping

GitLab merge requests map to `NodeKind::PullRequest` (same as GitHub PRs) for a unified forge model. MR names are prefixed `MR!{iid}`, GitLab issues use `GLIssue#{iid}`.

### Rate Limiting

GitLab API has rate limits (varies by instance). Homer:

1. Reads `ratelimit-remaining` header from responses
2. Warns when remaining requests drop below 10
3. Paginates results (100 per page)

### Incrementality

```
Checkpoint: gitlab_last_mr = 42, gitlab_last_issue = 87

On update:
1. Fetch MRs where iid > 42 (sorted ascending by created_at)
2. Fetch issues where iid > 87 (sorted ascending by created_at)
3. Update checkpoints to max iid values seen
```

### Configuration

```toml
[extraction.gitlab]
token_env = "GITLAB_TOKEN"         # Environment variable for auth token
max_mr_history = 500               # Max MRs to fetch on initial run (0 = unlimited)
max_issue_history = 1000           # Max issues to fetch on initial run (0 = unlimited)
include_comments = true            # Fetch MR/issue comments as metadata
include_reviews = true             # Fetch MR approvals and create Reviewed edges
```

### `has_work` Override

The GitLab extractor returns `false` from `has_work()` when no token is available (`GITLAB_TOKEN` not set), silently skipping extraction. This is the same behavior as the GitHub extractor — missing forge credentials are not an error.

---

## Structure Extractor

### What It Extracts

| Data | Node/Edge | Source |
|------|-----------|-------|
| Source files | `Node(File)` | File tree walk |
| Directories as modules | `Node(Module)` | Directory structure |
| File → module membership | `Hyperedge(BelongsTo)` | Path containment |
| Language detection | Metadata on File nodes | Extension mapping + content heuristics |
| Build/test commands | Metadata on root Module | CI config parsing |
| External dependencies | `Node(ExternalDep)` + `Hyperedge(DependsOn)` | Manifest parsing |

### CI Config Parsing

Homer extracts build, test, and lint commands from CI configuration:

| CI System | Config File | What to Extract |
|-----------|------------|----------------|
| GitHub Actions | `.github/workflows/*.yml` | `run:` commands in steps |
| GitLab CI | `.gitlab-ci.yml` | `script:` entries |
| Makefile | `Makefile` | Target names and commands |
| Package scripts | `package.json` | `scripts` object |
| Cargo | `Cargo.toml` | Implied `cargo build`, `cargo test` |
| Justfile | `justfile` / `Justfile` | Recipe names and commands |

These are stored as metadata on the root Module node and rendered into AGENTS.md as "how to build/test this project."

### Manifest Parsing

| Ecosystem | Manifest | Dependencies Extracted |
|-----------|----------|----------------------|
| Rust | `Cargo.toml` | `[dependencies]`, `[dev-dependencies]` |
| Node.js | `package.json` | `dependencies`, `devDependencies` |
| Python | `pyproject.toml`, `requirements.txt` | `[project.dependencies]`, `pip install` entries |
| Go | `go.mod` | `require` directives |
| Java | `pom.xml`, `build.gradle` | `<dependency>` elements, `implementation` entries |

### File Filtering

Not all files are relevant for code analysis. Homer filters by configuration:

```toml
[extraction.structure]
include_patterns = ["**/*.rs", "**/*.py", "**/*.ts", "**/*.tsx", "**/*.js", "**/*.go", "**/*.java"]
exclude_patterns = [
    "**/node_modules/**",
    "**/vendor/**", 
    "**/target/**",
    "**/.git/**",
    "**/dist/**",
    "**/build/**",
    "**/*.min.js",
    "**/*.generated.*",
]
```

### Incrementality

Structure extraction is checkpoint-gated:
- Checkpoint key: `structure_last_sha`
- If `structure_last_sha == git_last_sha`, extractor skips
- Otherwise, the extractor scans configured include patterns and upserts file/module nodes

---

## Graph Extractor

**See [GRAPH_ENGINE.md](GRAPH_ENGINE.md) for detailed graph engine specification.**

The graph extractor orchestrates the `homer-graphs` crate to build call graphs and import graphs. It operates in three tiers:

### Tier 1: Precise (Stack Graph Rules)

For languages with stack graph rules (Python, JavaScript, TypeScript, Java, and Homer-authored rules for Rust and Go):

1. Parse source file with tree-sitter
2. Execute TSG (tree-sitter-graph) rules to construct scope graph
3. Run path-stitching algorithm for cross-file name resolution
4. Project call graph: for each function call site, resolve to target definition
5. Project import graph: for each import statement, resolve to target file/module
6. **Extract doc comments**: Capture doc comments adjacent to function/type/module definitions and store as metadata on the corresponding node (see [STORE.md](STORE.md#doc-comment-metadata))

**Output**: High-confidence `Calls` and `Imports` hyperedges with `confidence: 1.0`, plus doc comment metadata on code entity nodes

### Tier 2: Heuristic (Tree-sitter AST Walking)

For languages without stack graph rules:

1. Parse source file with tree-sitter
2. Walk AST to find function definitions (tree-sitter query patterns per language)
3. Walk AST to find function call sites
4. Match call sites to definitions by name (within-file precise, cross-file approximate)
5. Walk AST to find import/include statements → map to files
6. **Extract doc comments**: Same as Tier 1 — capture adjacent comments on definitions

**Output**: Medium-confidence edges with `confidence: 0.6-0.8` (depending on resolution certainty), plus doc comment metadata

### Tier 3: Manifest (Package Dependencies Only)

For unsupported languages or when deeper analysis is skipped:

1. Parse package manifest
2. Record external dependency relationships

**Output**: Module-level `DependsOn` edges with `confidence: 1.0` (declared dependencies are certain)

### Doc Comment Extraction

Doc comments are extracted during the graph extraction pass — no separate file walk is needed. The graph extractor already visits every function/type definition; extracting the adjacent doc comment is a trivial addition:

```rust
/// During tree-sitter AST walk, for each definition node:
fn extract_doc_comment(tree: &Tree, source: &str, def_node: &Node) -> Option<DocCommentData> {
    // Look for comment nodes immediately preceding the definition
    // Strip syntax markers (///, /** */, #, """, etc.)
    // Detect doc style (rustdoc, jsdoc, numpy, etc.)
    // Return stripped text + hash + style
}
```

The extracted data is stored in the code entity node's metadata as `doc_comment`, `doc_comment_hash`, and `doc_style`. See [STORE.md](STORE.md#doc-comment-metadata) for the data model.

### Incrementality

Graph extraction tracks `graph_last_sha` checkpoint:

1. On update: identify files changed since checkpoint commit range
2. Parse only changed files for definitions/calls/imports
3. Run scope-graph resolution over the selected file set
4. Upsert resulting edges by deterministic hyperedge identity
5. Update `graph_last_sha` to current git checkpoint

---

## Document Extractor

### Motivation

Repositories contain significant knowledge in non-code artifacts that code analysis alone can't surface:

| Document Type | What It Contains | Why It Matters for Agents |
|---------------|-----------------|--------------------------|
| README.md | Project purpose, getting started, high-level architecture | Primary orientation context |
| CONTRIBUTING.md | Coding standards, PR process, review expectations | Process conventions |
| Architecture docs (ADRs, ARCHITECTURE.md) | Design decisions, trade-off rationale, system boundaries | *Why* the code is the way it is |
| API documentation | Interface contracts, usage examples, deprecation notices | Stable vs. volatile surfaces |
| Changelogs | Evolution narrative, breaking changes, migration paths | What changed and when |
| Wiki pages | Extended guides, runbooks, troubleshooting | Operational knowledge |
| Configuration guides | Environment setup, deployment, feature flags | Operational context |
| Topos spec files | Formal specifications in .tps format | Machine-readable intent |

**Key insight**: Doc comments on functions are especially valuable when combined with graph analysis. A function with PageRank 0.92 *and* a well-written doc comment gives Homer rich material for the semantic summary without needing an LLM call. A high-centrality function with *no* doc comment is a different kind of signal — it's important but undocumented, which is itself a risk flag.

### What the Document Extractor Produces

**For each documentation file:**

1. **`Node(Document)`** with metadata:
   - `doc_type: DocumentType`
   - `title`: extracted from first heading or filename
   - `sections`: list of heading/section names (for navigation)
   - `content_hash`: for incremental change detection
   - `word_count`: for sizing

2. **`Hyperedge(Documents)`** linking the document to code entities it references:
   - Parse Markdown links to source files
   - Parse backtick-quoted identifiers that match known function/type names
   - Parse file paths mentioned in prose
   - Confidence varies: explicit links = 1.0, name matches = 0.7, fuzzy = 0.4

### Specification

```rust
pub struct DocumentExtractor {
    /// Patterns for document detection
    doc_patterns: Vec<DocumentPattern>,
}

pub struct DocumentPattern {
    /// Glob pattern for matching files
    glob: String,
    /// What kind of document this represents
    doc_type: DocumentType,
    /// How to parse it
    parser: DocumentParser,
}

pub enum DocumentType {
    Readme,
    Contributing,
    Architecture,
    Adr,            // Architecture Decision Record
    Changelog,
    ApiDoc,
    Guide,
    Runbook,
    Other,
}

pub enum DocumentParser {
    /// Parse as Markdown, extract headings, links, code blocks
    Markdown,
    /// Parse as reStructuredText
    Rst,
    /// Parse as plain text
    PlainText,
    /// Parse as Topos spec (.tps files)
    Topos,
}
```

### Configuration

```toml
[extraction.documents]
enabled = true
include_doc_comments = true        # Extract doc comments during graph extraction
include_patterns = [
    "README*",
    "CONTRIBUTING*",
    "ARCHITECTURE*",
    "CHANGELOG*",
    "docs/**/*.md",
    "doc/**/*.md",
    "adr/**/*.md",
    "wiki/**/*.md",
    "*.tps",                        # Topos spec files
]
exclude_patterns = [
    "**/node_modules/**",
    "**/vendor/**",
]
```

### Input/Output Circularity

Homer *extracts* from existing AGENTS.md/CLAUDE.md files (treating them as agent rules or documents), and Homer *generates* AGENTS.md as an output artifact. This creates a circularity that must be handled explicitly:

1. **Never silently overwrite.** If a human-curated AGENTS.md or CLAUDE.md exists, Homer writes its generated version to a distinct path (e.g., `AGENTS.homer.md` or `.homer/agents.md`) by default.
2. **Diff mode.** `homer render --format agents-md --diff` produces a comparison: what Homer's analysis found that the existing file doesn't mention (blind spots), and what the human wrote that Homer can't validate from evidence (potentially stale claims).
3. **Merge mode.** `homer render --format agents-md --merge` produces a merged document that preserves human-curated sections (marked with `<!-- homer:preserve -->` comments) while inserting Homer-generated sections around them.
4. **If no existing file exists**, Homer writes directly to `AGENTS.md` (or the configured path) with no conflict.

This is the same pattern that formatters and linters use — don't destroy human work, augment it.

### Incrementality

Document extraction is checkpoint-gated:
1. Checkpoint key: `document_last_sha`
2. If unchanged from git checkpoint, extractor skips
3. When it runs, document nodes still use content hashes for idempotent node upserts

---

## Prompt and Agent Input Extractor

### Motivation

When developers use AI coding agents (Claude Code, Cursor, Windsurf, Cline, Copilot Chat), their interactions contain a layer of knowledge that exists nowhere else:

| Signal | What It Reveals | Example |
|--------|----------------|---------|
| Task descriptions | What developers actually work on, in their own words | "Add rate limiting to the auth endpoint" |
| Domain vocabulary | How humans name concepts vs. how code names them | "the order pipeline" (human) vs. `ProcessOrderWorkflow` (code) |
| Context provided by developers | What they think is important context | "Remember that this module handles both v1 and v2 API formats" |
| Corrections and follow-ups | Where agents misunderstand the codebase | "No, that function is deprecated, use the new one in auth/v2" |
| File references in prompts | What files humans consider related for a task | "Look at src/auth.rs and src/middleware/rate_limit.rs together" |
| Repeated patterns | Tasks that happen often enough to warrant skills | "Add a new API endpoint" appears in 30% of sessions |
| Failure modes | Where the codebase is confusing to agents | Same area gets corrections repeatedly |

**This is essentially a record of the human-codebase interface.** Mining it reveals what matters to developers, where the codebase confuses agents, and what patterns are common enough to codify.

### Sources

| Source | Location | Format | Content |
|--------|----------|--------|---------|
| Claude Code sessions | `.claude/` directory, JSONL logs | Structured JSON with role/content | Full conversation history |
| Claude Code CLAUDE.md | `CLAUDE.md`, `.claude/settings.json` | Markdown, JSON | Human-curated agent context |
| Cursor rules | `.cursor/rules/*.mdc` | Markdown with frontmatter | IDE-specific agent instructions |
| Windsurf rules | `.windsurf/rules/*.md` | Markdown | IDE-specific agent instructions |
| Cline rules | `.clinerules/*.md` | Markdown | IDE-specific agent instructions |
| Git commit messages | `.git` | Text | Often contain "Co-authored-by: AI" markers |
| MCP interaction logs | Varies by client | JSON | Tool calls and results |
| `.beads/` transcripts | `.beads/` directory | Markdown/structured text | Conversation context snapshots |

### Specification

```rust
pub struct PromptExtractor {
    sources: Vec<Box<dyn PromptSource>>,
}

/// Trait for different prompt/session log formats
pub trait PromptSource: Send + Sync {
    fn name(&self) -> &str;
    fn detect(&self, repo_path: &Path) -> bool;
    fn extract(&self, repo_path: &Path, since: Option<DateTime<Utc>>) -> Result<Vec<AgentInteraction>>;
}

/// A single interaction with an AI agent
pub struct AgentInteraction {
    /// Which tool/IDE this came from
    pub source: AgentSource,
    /// Session identifier (groups related prompts)
    pub session_id: Option<String>,
    /// The human's input
    pub prompt_text: String,
    /// Files explicitly referenced in the prompt
    pub referenced_files: Vec<PathBuf>,
    /// Files modified as a result (if trackable)
    pub modified_files: Vec<PathBuf>,
    /// Timestamp
    pub timestamp: DateTime<Utc>,
    /// Whether the interaction included corrections
    pub had_corrections: bool,
    /// Tags extracted from the interaction (task type, area, etc.)
    pub tags: Vec<String>,
}

pub enum AgentSource {
    ClaudeCode,
    Cursor,
    Windsurf,
    Cline,
    CopilotChat,
    Unknown(String),
}
```

### Concrete Source Implementations

**Claude Code sessions:**
```rust
struct ClaudeCodeSource;

impl PromptSource for ClaudeCodeSource {
    fn detect(&self, repo_path: &Path) -> bool {
        repo_path.join(".claude").exists()
    }
    
    fn extract(&self, repo_path: &Path, since: Option<DateTime<Utc>>) -> Result<Vec<AgentInteraction>> {
        // Parse JSONL session logs in .claude/
        // Extract user messages (role: "user")
        // Detect file references (paths mentioned in text, tool_use blocks)
        // Detect corrections (see Correction Detection below)
        // Group by session
    }
}
```

**Agent rule files (CLAUDE.md, .cursor/rules, etc.):**
```rust
struct AgentRuleSource;

impl PromptSource for AgentRuleSource {
    fn detect(&self, repo_path: &Path) -> bool {
        repo_path.join("CLAUDE.md").exists()
            || repo_path.join("AGENTS.md").exists()
            || repo_path.join(".cursor/rules").exists()
            || repo_path.join(".windsurf/rules").exists()
            || repo_path.join(".clinerules").exists()
    }
    
    fn extract(&self, repo_path: &Path, since: Option<DateTime<Utc>>) -> Result<Vec<AgentInteraction>> {
        // These aren't prompts per se — they're curated context
        // Extract as AgentRule nodes, not Prompt nodes
        // Parse for: file references, convention statements, warnings
        // Track changes over time (git history of these files = evolving understanding)
    }
}
```

**Beads transcripts:**
```rust
struct BeadsSource;

impl PromptSource for BeadsSource {
    fn detect(&self, repo_path: &Path) -> bool {
        repo_path.join(".beads").exists()
    }
    
    fn extract(&self, repo_path: &Path, since: Option<DateTime<Utc>>) -> Result<Vec<AgentInteraction>> {
        // Parse .beads/ conversation transcripts
        // Extract human turns as prompts
        // Map referenced files
        // Track session continuity
    }
}
```

### Prompt-to-Commit Correlation

The highest-value signal from prompt mining comes from connecting agent interactions to their *outcomes* in the commit history. If Homer knows that a Claude Code session at time T produced commits C1, C2, C3, it can:

1. Create high-confidence `PromptModifiedFiles` edges (not just "what was discussed" but "what was changed")
2. Compare the intent expressed in the prompt against the actual changes made — divergence is a signal
3. Build much stronger task pattern models (prompt → files touched → outcome)

**Detection methods** (in order of confidence):
- **Explicit tool_use blocks:** Claude Code sessions contain `tool_use` blocks with file paths and edit content. Direct correlation, confidence 1.0.
- **Commit attribution:** Commits with `Co-authored-by: Claude` or similar markers, within the session time window. Confidence 0.9.
- **Timestamp proximity:** Commits made within N minutes of a session's end, touching files mentioned in the session. Confidence 0.6-0.8 depending on overlap.

### Correction Detection

The naive approach (keyword matching for "no", "that's wrong") is fragile. Better heuristics:

1. **Edit-after-response patterns:** If an agent modifies files and the user immediately follows with another prompt referencing the same files, that's a correction signal regardless of wording. The session structure gives you this for free — look for user turns that follow agent actions and reference the same file set.

2. **Revert-and-redo patterns:** If a session contains a sequence like (agent edits file A) → (user says something) → (agent edits file A again), the second edit likely corrects the first.

3. **Explicit rejection markers:** `/undo`, `git checkout`, `git stash` commands appearing in session logs after agent actions.

4. **Sentiment shift in follow-ups:** LLM-assisted (salience-gated, like everything else): for high-frequency correction areas, use an LLM to classify whether follow-up prompts express correction, refinement, or continuation. Cache the classifier results.

### Privacy and Sensitivity

Prompt data is sensitive. Homer must handle it carefully:

```toml
[extraction.prompts]
enabled = false                     # Opt-in, not opt-out
sources = ["claude-code", "agent-rules"]  # Which sources to mine
redact_sensitive = true             # Strip API keys, passwords, personal info detected in prompts
store_full_text = false             # If false, only store extracted metadata (references, patterns), not raw prompt text
hash_session_ids = true             # Don't store raw session identifiers
exclude_contributors = []           # Skip automated agent interactions (CI bots, automated PR reviews)
```

When `store_full_text = false`, Homer extracts structured data (file references, task patterns, correction signals) but discards the raw prompt text. This preserves privacy while retaining analytical value.

**Agent rule files** (CLAUDE.md, .cursor/rules) are not sensitive — they're committed to the repo and are public. These are always extracted when detected, regardless of the `enabled` flag for prompt mining.

### Incrementality

- Checkpoint key: `prompt_last_sha`
- If unchanged from git checkpoint, prompt extraction skips
- Agent rule/session nodes still use content hashes to avoid node churn when content is unchanged
