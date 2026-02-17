# Homer Integrations

> Connections to Topos, Ananke, Loop, and the broader agentic development ecosystem.

**Parent**: [README.md](README.md)  
**Related**: [RENDERERS.md](RENDERERS.md) · [CLI.md](CLI.md) · [EVOLUTION.md](EVOLUTION.md)

---

## Integration Philosophy

Homer stands alone and provides value independently. Integrations with parallel projects are at well-defined boundaries: artifact-level (Homer emits files that other tools consume), query-level (other tools call Homer's MCP tools), or shared-technology-level (common crates and patterns). No circular dependencies.

```
                    ┌─────────┐
                    │  Topos  │ ← Homer emits .tps specs, feeds drift detection
                    └────┬────┘
                         │ artifact + query boundary
    ┌─────────┐    ┌─────┴─────┐    ┌─────────┐
    │  Ananke │    │   Homer   │    │  Loop   │
    │(future) │    │           │    │(memory) │
    └─────────┘    └─────┬─────┘    └─────────┘
                         │
              ┌──────────┴──────────┐
              │                     │
        ┌─────┴─────┐        ┌─────┴─────┐
        │ Claude Code│        │   MCP     │
        │  Skills    │        │  Clients  │
        └───────────┘        └───────────┘
```

---

## Topos Integration

**Project**: [github.com/rand/topos](https://github.com/rand/topos)  
**Nature**: A semantic contract language for human-AI collaboration. CommonMark-compatible, with typed holes, soft constraints, traceability, and drift detection.

### What Homer Does for Topos

**1. Reverse-engineered specification generation**

Homer's spec renderer ([RENDERERS.md](RENDERERS.md#renderer-4-specification-topos-format)) emits `.tps` files — closing the loop for brownfield codebases that have code but no specification.

```
Brownfield repo (no spec) → Homer mines → .tps spec → Human refines → Living contract
```

The mapping:

| Homer Analysis | Topos Construct |
|---------------|----------------|
| Community clusters + semantic summaries | `# Design` sections (functional areas) |
| High-salience types + field analysis | `Concept` blocks with typed fields |
| Stable interface patterns | `Behavior` blocks with `ensures`/`requires` |
| Never-violated constraints from history | `Invariant` blocks |
| Issue/PR → requirement inference | `# Requirements` with acceptance criteria |
| Implementation with evidence | `## TASK-N` with evidence (PR, commit) |
| Uncertain/incomplete analysis | `[?]` typed holes |
| Approximate/aesthetic patterns | `[~]` soft constraints |

**2. Rich drift detection data**

Topos has `topos drift` (structural comparison + optional LLM-as-Judge). Homer's deep analysis makes drift detection dramatically more precise:

- Homer knows which functions implement which concepts → detect when a Concept's implementing code diverges from spec
- Homer tracks co-change patterns → detect when a Task's file list doesn't match actual change sets  
- Homer measures graph evolution → detect architectural drift (spec says modules are independent, but Homer shows coupling increasing)

**Integration mechanism**: Topos calls Homer MCP tools:

```
topos drift src/auth/ 
  → calls homer_query("src/auth/validate.rs") 
  → compares Homer's analysis with spec's Behavior blocks
  → reports divergence with evidence
```

**3. Context enrichment for Topos context compiler**

When `topos context TASK-17 --format cursor` generates AI context, it could pull Homer data for the relevant files: "by the way, `validate_order` has PageRank 0.89 and 47 callers — be careful."

### What Topos Does for Homer

**1. Structured input for semantic analysis**

If a Topos spec exists, Homer can use it to seed its understanding instead of relying solely on code mining. The spec provides human-verified intent that enriches Homer's semantic summaries.

**2. Validation of Homer's analysis**

When Homer generates a `.tps` spec and a human refines it, the delta between Homer's output and the human correction is signal about where Homer's analysis was wrong. This feedback loop could improve prompts over time.

### Boundary Rules

- Homer does **not** depend on the `topos-*` crates at compile time
- Homer's spec renderer produces valid `.tps` files by implementing the format directly (it's CommonMark-compatible text with structured extensions — no parser needed to *generate*)
- Topos consumes Homer via MCP tools (standard protocol) or by reading Homer's output files
- Both tools can operate independently — Homer without Topos, Topos without Homer

---

## Ananke Integration

**Project**: Ananke (constraint-driven code generation framework)  
**Components**: Clew (constraint extraction), Braid (constraint compilation), Maze (orchestration)  
**Nature**: Treats AI code generation as constrained search through valid programs

### How Homer Feeds Ananke (Future)

Homer's analysis produces exactly the kinds of constraints that Ananke's Clew component needs to extract:

| Homer Output | Ananke Constraint Type |
|-------------|----------------------|
| Stability classification (`StableCore`) | Interface conformance constraint: "do not modify this interface" |
| Never-violated import boundaries | Module boundary constraint: "module A never imports from module B" |
| Co-change patterns | Completeness constraint: "if you change A, you must also change B" |
| Naming conventions | Style constraint: "functions follow `snake_case`, types follow `PascalCase`" |
| Testing patterns | Evidence constraint: "every public function has a corresponding test" |
| Call graph structure | Architectural constraint: "data flows A → B → C, never C → A" |

Homer could export a constraint specification that Ananke consumes, enabling constraint-driven generation that respects the historical invariants of the codebase.

### Boundary Rules

- Homer does **not** depend on Ananke
- Homer does **not** emit Ananke-specific formats (initially)
- The connection is philosophical and data-compatible, not code-coupled
- A future `homer render --format ananke-constraints` renderer could bridge them

---

## Loop Integration

**Project**: [github.com/rand/loop](https://github.com/rand/loop)  
**Nature**: Unified RLM (Recursive Language Model) orchestration — context management, hypergraph memory, reasoning traces

### Shared Concepts

Homer's hypergraph store design is directly inspired by Loop's hypergraph memory:
- SQLite-backed persistence
- Tiered lifecycle (freshness tracking, invalidation)
- N-ary relationships as first-class citizens

### Potential Integration

Loop's agents could consume Homer's knowledge base as long-term memory about a codebase. When a Loop-orchestrated agent starts working on a repository, it could:

1. Query Homer MCP tools for codebase understanding
2. Use Homer's risk map to calibrate caution
3. Reference Homer's co-change patterns to ensure completeness
4. Draw on Homer's semantic summaries for context about load-bearing code

This is a consumer relationship — Loop agents consume Homer data, but Homer doesn't depend on Loop.

### Shared Technology

Both projects use:
- Rust as the implementation language
- SQLite for persistence
- Hypergraph data modeling
- MCP for tool integration

There may be opportunity to extract shared crates (e.g., a `hypergraph-sqlite` crate) if the overlap is substantial, but this is a premature optimization. Build independently first, factor out shared code later if it emerges naturally.

---

## Claude Code Integration

Homer integrates with Claude Code through two mechanisms:

### 1. AGENTS.md

Claude Code reads `AGENTS.md` (or `CLAUDE.md`) at the repository root for project context. Homer generates this file with richer content than Claude Code's built-in `/init`:

| Feature | `/init` | Homer AGENTS.md |
|---------|---------|-----------------|
| Build/test commands | ✓ (from config files) | ✓ (from config files + CI) |
| Directory structure | ✓ (current state) | ✓ (with purpose annotations) |
| Key abstractions | ✗ | ✓ (from graph analysis) |
| Load-bearing code | ✗ | ✓ (from centrality analysis) |
| Change patterns | ✗ | ✓ (from co-change analysis) |
| Danger zones | ✗ | ✓ (from risk analysis) |
| Naming conventions | ✗ | ✓ (from convention analysis) |
| Domain vocabulary | ✗ | ✓ (from identifier analysis) |

### 2. Skills

Homer generates Claude Code skills (`.md` files in `.claude/skills/`) that encode repo-specific change patterns. These teach Claude Code *how this repo works* — not generic best practices, but the actual patterns derived from historical changes.

### 3. MCP Tools

When Claude Code has MCP tools configured, Homer's `homer serve` command exposes:
- `homer_query`: Entity lookup
- `homer_graph`: Centrality metrics
- `homer_risk`: Risk assessment for files being modified
- `homer_co_changes`: Co-change analysis for completeness checking
- `homer_conventions`: Convention guidance per-module

Claude Code can call these tools during coding sessions to make informed decisions.

---

## Generic AI Agent Integration

Homer's MCP tools work with any MCP-compatible AI agent, not just Claude Code:

- **Cursor**: Via MCP tool integration
- **Windsurf**: Via MCP tool integration
- **Cline**: Via MCP tool integration
- **Custom agents**: Any agent that speaks MCP

The MCP interface is the universal integration surface. The generated files (AGENTS.md, module context, risk map) work with any agent that reads project files.

---

## CI/CD Integration

Homer can run in CI pipelines:

```yaml
# .github/workflows/homer.yml
name: Homer Analysis
on:
  push:
    branches: [main]
  release:
    types: [published]

jobs:
  analyze:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0  # Homer needs full git history
      
      - name: Install Homer
        run: cargo install homer-cli
      
      - name: Update analysis
        run: homer update
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}
      
      - name: Generate artifacts
        run: homer render --all
      
      - name: Check risk
        run: |
          # Fail if high-risk files were modified without tests
          homer risk-check --changed-files "${{ github.event.pull_request.changed_files }}"
      
      - name: Upload report
        uses: actions/upload-artifact@v4
        with:
          name: homer-report
          path: homer-report.html
```

### PR Risk Check

A `homer risk-check` command (or CI-specific wrapper) could:
1. Read the list of changed files from the PR
2. Look up risk levels from the store
3. Exit with non-zero code if high-risk files are modified without corresponding test changes
4. Post a PR comment summarizing the risk assessment
