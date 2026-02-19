// Convention analysis: naming, testing, error handling, documentation style, agent rule validation.
// All analysis is purely algorithmic — no LLM calls.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use chrono::Utc;
use serde::Serialize;
use tracing::{info, instrument};

use crate::config::HomerConfig;
use crate::store::HomerStore;
use crate::types::{AnalysisKind, AnalysisResult, AnalysisResultId, NodeFilter, NodeKind};

use super::AnalyzeStats;
use super::traits::Analyzer;

#[derive(Debug)]
pub struct ConventionAnalyzer {
    repo_path: std::path::PathBuf,
}

impl ConventionAnalyzer {
    pub fn new(repo_path: &Path) -> Self {
        Self {
            repo_path: repo_path.to_path_buf(),
        }
    }
}

#[async_trait::async_trait]
impl Analyzer for ConventionAnalyzer {
    fn name(&self) -> &'static str {
        "convention"
    }

    #[instrument(skip_all, name = "convention_analyze")]
    async fn analyze(
        &self,
        store: &dyn HomerStore,
        _config: &HomerConfig,
    ) -> crate::error::Result<AnalyzeStats> {
        let start = Instant::now();
        let mut stats = AnalyzeStats::default();

        // Find the root module node to store project-wide results
        let root_node = find_root_module(store).await?;
        let Some(root_id) = root_node else {
            info!("No root module node found, skipping convention analysis");
            return Ok(stats);
        };

        // Analyze naming patterns from function and type names
        let naming = analyze_naming(store).await?;
        store_result(store, root_id, AnalysisKind::NamingPattern, &naming).await?;
        stats.results_stored += 1;

        // Analyze testing patterns from file structure and metadata
        let testing = analyze_testing(store, &self.repo_path).await?;
        store_result(store, root_id, AnalysisKind::TestingPattern, &testing).await?;
        stats.results_stored += 1;

        // Analyze error handling patterns
        let errors = analyze_error_handling(store, &self.repo_path).await?;
        store_result(store, root_id, AnalysisKind::ErrorHandlingPattern, &errors).await?;
        stats.results_stored += 1;

        // Analyze documentation style from doc comments
        let doc_style = analyze_doc_style(store).await?;
        store_result(
            store,
            root_id,
            AnalysisKind::DocumentationStylePattern,
            &doc_style,
        )
        .await?;
        stats.results_stored += 1;

        // Validate agent rules against actual patterns
        let agent_rules = validate_agent_rules(&self.repo_path, &naming);
        store_result(
            store,
            root_id,
            AnalysisKind::AgentRuleValidation,
            &agent_rules,
        )
        .await?;
        stats.results_stored += 1;

        stats.duration = start.elapsed();
        info!(
            results = stats.results_stored,
            duration = ?stats.duration,
            "Convention analysis complete"
        );

        Ok(stats)
    }
}

// ── Naming Pattern Analysis ─────────────────────────────────────────

#[derive(Debug, Serialize)]
struct NamingResult {
    conventions: Vec<ConventionEntry>,
    dominant: String,
    adherence_rate: f64,
    common_prefixes: Vec<(String, u32)>,
    common_suffixes: Vec<(String, u32)>,
}

#[derive(Debug, Serialize)]
struct ConventionEntry {
    convention: String,
    count: u32,
    percentage: f64,
}

async fn analyze_naming(store: &dyn HomerStore) -> crate::error::Result<NamingResult> {
    let fn_filter = NodeFilter {
        kind: Some(NodeKind::Function),
        ..Default::default()
    };
    let type_filter = NodeFilter {
        kind: Some(NodeKind::Type),
        ..Default::default()
    };

    let functions = store.find_nodes(&fn_filter).await?;
    let types = store.find_nodes(&type_filter).await?;

    let mut counts: HashMap<&str, u32> = HashMap::new();
    let mut prefix_counts: HashMap<String, u32> = HashMap::new();
    let mut suffix_counts: HashMap<String, u32> = HashMap::new();

    // Analyze function names (extract leaf name from qualified path)
    for func in &functions {
        let leaf = leaf_name(&func.name);
        classify_and_count(leaf, &mut counts, &mut prefix_counts, &mut suffix_counts);
    }

    // Analyze type names
    for typ in &types {
        let leaf = leaf_name(&typ.name);
        let convention = classify_name(leaf);
        *counts.entry(convention).or_default() += 1;
    }

    let total: u32 = counts.values().sum();
    let mut conventions: Vec<ConventionEntry> = counts
        .iter()
        .map(|(name, count)| ConventionEntry {
            convention: (*name).to_string(),
            count: *count,
            percentage: if total > 0 {
                f64::from(*count) / f64::from(total) * 100.0
            } else {
                0.0
            },
        })
        .collect();
    conventions.sort_by(|a, b| b.count.cmp(&a.count));

    let dominant = conventions
        .first()
        .map_or("unknown", |c| c.convention.as_str())
        .to_string();
    let adherence = conventions.first().map_or(0.0, |c| c.percentage / 100.0);

    let mut top_prefixes: Vec<_> = prefix_counts.into_iter().collect();
    top_prefixes.sort_by(|a, b| b.1.cmp(&a.1));
    top_prefixes.truncate(10);

    let mut top_suffixes: Vec<_> = suffix_counts.into_iter().collect();
    top_suffixes.sort_by(|a, b| b.1.cmp(&a.1));
    top_suffixes.truncate(10);

    Ok(NamingResult {
        conventions,
        dominant,
        adherence_rate: adherence,
        common_prefixes: top_prefixes,
        common_suffixes: top_suffixes,
    })
}

fn leaf_name(qualified: &str) -> &str {
    qualified.rsplit("::").next().unwrap_or(qualified)
}

fn classify_name(name: &str) -> &'static str {
    if name.is_empty() {
        return "unknown";
    }
    // Check for snake_case: lowercase with underscores
    if name.contains('_') && name == name.to_lowercase() {
        return "snake_case";
    }
    // Check for SCREAMING_SNAKE_CASE
    if name.contains('_') && name == name.to_uppercase() {
        return "SCREAMING_SNAKE_CASE";
    }
    // Check for PascalCase: starts with uppercase, no underscores
    if name.starts_with(|c: char| c.is_uppercase()) && !name.contains('_') {
        return "PascalCase";
    }
    // Check for camelCase: starts with lowercase, has uppercase chars, no underscores
    if name.starts_with(|c: char| c.is_lowercase())
        && name.chars().any(char::is_uppercase)
        && !name.contains('_')
    {
        return "camelCase";
    }
    // All lowercase, no underscores
    if name == name.to_lowercase() && !name.contains('_') {
        return "lowercase";
    }
    "mixed"
}

fn classify_and_count(
    name: &str,
    counts: &mut HashMap<&str, u32>,
    prefixes: &mut HashMap<String, u32>,
    suffixes: &mut HashMap<String, u32>,
) {
    let convention = classify_name(name);
    *counts.entry(convention).or_default() += 1;

    // Extract common prefixes (get_, set_, is_, has_, etc.)
    for prefix in [
        "get_", "set_", "is_", "has_", "new_", "from_", "to_", "with_", "create_", "delete_",
        "update_", "find_", "test_", "handle_",
    ] {
        if name.starts_with(prefix) {
            *prefixes.entry(prefix.to_string()).or_default() += 1;
            break;
        }
    }

    // Extract common suffixes (Handler, Service, Error, Result, Test, etc.)
    for suffix in [
        "_test", "_spec", "_impl", "_handler", "_service", "_error", "_result", "_factory",
        "_builder", "_config", "_utils",
    ] {
        if name.ends_with(suffix) {
            *suffixes.entry(suffix.to_string()).or_default() += 1;
            break;
        }
    }
}

// ── Testing Pattern Analysis ────────────────────────────────────────

#[derive(Debug, Serialize)]
struct TestingResult {
    framework: Option<String>,
    test_file_pattern: String,
    co_located: bool,
    test_count: u32,
    source_count: u32,
    test_ratio: f64,
}

async fn analyze_testing(
    store: &dyn HomerStore,
    repo_path: &Path,
) -> crate::error::Result<TestingResult> {
    let file_filter = NodeFilter {
        kind: Some(NodeKind::File),
        ..Default::default()
    };
    let files = store.find_nodes(&file_filter).await?;

    let mut test_files = 0u32;
    let mut source_files = 0u32;

    // Detect test file patterns
    let mut has_tests_dir = false;
    let mut has_test_prefix = false;
    let mut has_test_suffix = false;

    for file in &files {
        let name = &file.name;
        let is_test = name.contains("/tests/")
            || name.contains("/test/")
            || name.contains("_test.")
            || name.contains("test_")
            || name.contains(".test.")
            || name.contains(".spec.")
            || name.contains("_spec.");

        if is_test {
            test_files += 1;
            if name.contains("/tests/") || name.contains("/test/") {
                has_tests_dir = true;
            }
            if name.contains("test_") {
                has_test_prefix = true;
            }
            if name.contains("_test.") || name.contains(".test.") || name.contains(".spec.") {
                has_test_suffix = true;
            }
        } else if is_source_file(name) {
            source_files += 1;
        }
    }

    // Determine dominant test pattern
    let test_pattern = if has_tests_dir {
        "tests/ directory".to_string()
    } else if has_test_suffix {
        "*_test.* / *.test.* / *.spec.*".to_string()
    } else if has_test_prefix {
        "test_*".to_string()
    } else {
        "unknown".to_string()
    };

    // Check for co-location (test files alongside source files)
    let co_located = !has_tests_dir && (has_test_suffix || has_test_prefix);

    // Detect test framework from manifests
    let framework = detect_test_framework(repo_path);

    let test_ratio = if source_files > 0 {
        f64::from(test_files) / f64::from(source_files)
    } else {
        0.0
    };

    Ok(TestingResult {
        framework,
        test_file_pattern: test_pattern,
        co_located,
        test_count: test_files,
        source_count: source_files,
        test_ratio,
    })
}

fn is_source_file(name: &str) -> bool {
    let ext_list = [".rs", ".py", ".ts", ".tsx", ".js", ".jsx", ".go", ".java"];
    ext_list.iter().any(|ext| name.ends_with(ext))
}

fn detect_test_framework(repo_path: &Path) -> Option<String> {
    // Check Cargo.toml for Rust test framework
    if let Ok(content) = std::fs::read_to_string(repo_path.join("Cargo.toml")) {
        if content.contains("[dev-dependencies]") {
            return Some("cargo test".to_string());
        }
        // Even without dev-deps, Rust projects use cargo test by default
        if content.contains("[package]") {
            return Some("cargo test".to_string());
        }
    }

    // Check package.json for JS/TS test framework
    if let Ok(content) = std::fs::read_to_string(repo_path.join("package.json")) {
        if content.contains("\"jest\"") {
            return Some("jest".to_string());
        }
        if content.contains("\"vitest\"") {
            return Some("vitest".to_string());
        }
        if content.contains("\"mocha\"") {
            return Some("mocha".to_string());
        }
    }

    // Check for pytest (pyproject.toml or setup.cfg)
    if let Ok(content) = std::fs::read_to_string(repo_path.join("pyproject.toml")) {
        if content.contains("[tool.pytest") || content.contains("pytest") {
            return Some("pytest".to_string());
        }
    }

    // Check for Go test
    if repo_path.join("go.mod").exists() {
        return Some("go test".to_string());
    }

    None
}

// ── Error Handling Pattern Analysis ─────────────────────────────────

#[derive(Debug, Serialize)]
struct ErrorHandlingResult {
    approach: String,
    patterns: Vec<ErrorPattern>,
    dominant_pattern: String,
}

#[derive(Debug, Serialize)]
struct ErrorPattern {
    pattern: String,
    count: u32,
    language: String,
}

async fn analyze_error_handling(
    store: &dyn HomerStore,
    repo_path: &Path,
) -> crate::error::Result<ErrorHandlingResult> {
    let file_filter = NodeFilter {
        kind: Some(NodeKind::File),
        ..Default::default()
    };
    let files = store.find_nodes(&file_filter).await?;

    let mut pattern_counts: HashMap<String, u32> = HashMap::new();
    let mut patterns = Vec::new();

    for file in &files {
        let file_path = repo_path.join(&file.name);
        let lang = file
            .metadata
            .get("language")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");

        let Ok(source) = std::fs::read_to_string(&file_path) else {
            continue;
        };

        let detected = detect_error_patterns(&source, lang);
        for (pattern, count) in detected {
            *pattern_counts.entry(pattern.clone()).or_default() += count;
            patterns.push(ErrorPattern {
                pattern,
                count,
                language: lang.to_string(),
            });
        }
    }

    let dominant = pattern_counts
        .iter()
        .max_by_key(|(_, v)| *v)
        .map_or("unknown", |(k, _)| k.as_str())
        .to_string();

    let approach = if dominant.contains("Result") || dominant.contains('?') {
        "Result/Option type".to_string()
    } else if dominant.contains("try") || dominant.contains("except") {
        "Exceptions".to_string()
    } else if dominant.contains("if err") {
        "Error return codes".to_string()
    } else {
        "mixed".to_string()
    };

    // Deduplicate patterns: aggregate by pattern name
    let mut agg: Vec<ErrorPattern> = pattern_counts
        .into_iter()
        .map(|(pattern, count)| ErrorPattern {
            pattern,
            count,
            language: String::new(),
        })
        .collect();
    agg.sort_by(|a, b| b.count.cmp(&a.count));

    Ok(ErrorHandlingResult {
        approach,
        patterns: agg,
        dominant_pattern: dominant,
    })
}

fn detect_error_patterns(source: &str, lang: &str) -> Vec<(String, u32)> {
    let mut results = Vec::new();

    match lang {
        "rust" => {
            let q_ops = source.matches('?').count() as u32;
            if q_ops > 0 {
                results.push(("? operator".to_string(), q_ops));
            }
            let unwraps = count_pattern(source, ".unwrap()");
            if unwraps > 0 {
                results.push((".unwrap()".to_string(), unwraps));
            }
            let expects = count_pattern(source, ".expect(");
            if expects > 0 {
                results.push((".expect()".to_string(), expects));
            }
            let results_type = count_pattern(source, "Result<");
            if results_type > 0 {
                results.push(("Result<T, E>".to_string(), results_type));
            }
        }
        "python" => {
            let try_except = count_pattern(source, "except ");
            if try_except > 0 {
                results.push(("try/except".to_string(), try_except));
            }
            let raises = count_pattern(source, "raise ");
            if raises > 0 {
                results.push(("raise".to_string(), raises));
            }
        }
        "typescript" | "javascript" => {
            let try_catch = count_pattern(source, "catch ");
            if try_catch > 0 {
                results.push(("try/catch".to_string(), try_catch));
            }
            let throws = count_pattern(source, "throw ");
            if throws > 0 {
                results.push(("throw".to_string(), throws));
            }
        }
        "go" => {
            let if_err = count_pattern(source, "if err != nil");
            if if_err > 0 {
                results.push(("if err != nil".to_string(), if_err));
            }
        }
        _ => {}
    }

    results
}

fn count_pattern(source: &str, pattern: &str) -> u32 {
    source.matches(pattern).count() as u32
}

// ── Documentation Style Analysis ────────────────────────────────────

#[derive(Debug, Serialize)]
struct DocStyleResult {
    styles: Vec<StyleEntry>,
    dominant_style: String,
    documented_count: u32,
    total_entities: u32,
    coverage_rate: f64,
    documents_params: bool,
    documents_returns: bool,
    documents_examples: bool,
}

#[derive(Debug, Serialize)]
struct StyleEntry {
    style: String,
    count: u32,
    percentage: f64,
}

async fn analyze_doc_style(store: &dyn HomerStore) -> crate::error::Result<DocStyleResult> {
    let fn_filter = NodeFilter {
        kind: Some(NodeKind::Function),
        ..Default::default()
    };
    let type_filter = NodeFilter {
        kind: Some(NodeKind::Type),
        ..Default::default()
    };

    let functions = store.find_nodes(&fn_filter).await?;
    let types = store.find_nodes(&type_filter).await?;

    let total = (functions.len() + types.len()) as u32;
    let mut style_counts: HashMap<String, u32> = HashMap::new();
    let mut documented = 0u32;

    for node in functions.iter().chain(types.iter()) {
        if node.metadata.contains_key("doc_comment") {
            documented += 1;
            let style = node
                .metadata
                .get("doc_style")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            *style_counts.entry(style).or_default() += 1;
        }
    }

    let mut styles: Vec<StyleEntry> = style_counts
        .iter()
        .map(|(style, count)| StyleEntry {
            style: style.clone(),
            count: *count,
            percentage: if documented > 0 {
                f64::from(*count) / f64::from(documented) * 100.0
            } else {
                0.0
            },
        })
        .collect();
    styles.sort_by(|a, b| b.count.cmp(&a.count));

    let dominant = styles
        .first()
        .map_or("none", |s| s.style.as_str())
        .to_string();

    let coverage_rate = if total > 0 {
        f64::from(documented) / f64::from(total)
    } else {
        0.0
    };

    // Detect doc comment content patterns from file metadata
    let file_filter = NodeFilter {
        kind: Some(NodeKind::File),
        ..Default::default()
    };
    let files = store.find_nodes(&file_filter).await?;

    let mut has_param_docs = false;
    let mut has_return_docs = false;
    let mut has_example_docs = false;

    for file in &files {
        if let Some(doc_data) = file.metadata.get("doc_patterns") {
            if doc_data.get("params").and_then(serde_json::Value::as_bool) == Some(true) {
                has_param_docs = true;
            }
            if doc_data.get("returns").and_then(serde_json::Value::as_bool) == Some(true) {
                has_return_docs = true;
            }
            if doc_data
                .get("examples")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
            {
                has_example_docs = true;
            }
        }
        // Also check language-specific doc patterns from the metadata
        let lang = file
            .metadata
            .get("language")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        detect_doc_patterns_from_lang(
            lang,
            &mut has_param_docs,
            &mut has_return_docs,
            &mut has_example_docs,
        );
    }

    Ok(DocStyleResult {
        styles,
        dominant_style: dominant,
        documented_count: documented,
        total_entities: total,
        coverage_rate,
        documents_params: has_param_docs,
        documents_returns: has_return_docs,
        documents_examples: has_example_docs,
    })
}

/// Infer doc patterns from language (as a heuristic baseline).
fn detect_doc_patterns_from_lang(
    lang: &str,
    params: &mut bool,
    returns: &mut bool,
    examples: &mut bool,
) {
    match lang {
        "rust" => {
            // Rustdoc: # Arguments, # Returns, # Examples
            *params = true;
            *returns = true;
            *examples = true;
        }
        "python" => {
            // Numpy/Google style: Args:, Returns:, Examples:
            *params = true;
            *returns = true;
            *examples = true;
        }
        "typescript" | "javascript" => {
            // JSDoc: @param, @returns, @example
            *params = true;
            *returns = true;
        }
        "java" => {
            // Javadoc: @param, @return, @see
            *params = true;
            *returns = true;
        }
        _ => {}
    }
}

// ── Agent Rule Validation ───────────────────────────────────────────

#[derive(Debug, Serialize)]
struct AgentRuleResult {
    rule_files_found: Vec<String>,
    validated: Vec<ValidatedRule>,
    drifted: Vec<DriftedRule>,
    undocumented: Vec<UndocumentedPattern>,
}

#[derive(Debug, Serialize)]
struct UndocumentedPattern {
    pattern: String,
    description: String,
}

#[derive(Debug, Serialize)]
struct ValidatedRule {
    source: String,
    rule: String,
    actual: String,
}

#[derive(Debug, Serialize)]
struct DriftedRule {
    source: String,
    stated_convention: String,
    actual_pattern: String,
    adherence_rate: f64,
}

fn validate_agent_rules(repo_path: &Path, naming: &NamingResult) -> AgentRuleResult {
    let mut rule_files = Vec::new();
    let mut validated = Vec::new();
    let mut drifted = Vec::new();
    let mut rule_content_found = false;

    // Check known agent rule file locations
    let candidates = [
        "CLAUDE.md",
        ".claude/settings.json",
        ".cursor/rules/style.mdc",
        ".cursor/rules/naming.mdc",
        ".windsurf/rules/style.md",
        ".clinerules/conventions.md",
    ];

    for candidate in candidates {
        let path = repo_path.join(candidate);
        if path.exists() {
            rule_files.push((*candidate).to_string());
            rule_content_found = true;

            if let Ok(content) = std::fs::read_to_string(&path) {
                check_naming_rules(&content, candidate, naming, &mut validated, &mut drifted);
            }
        }
    }

    // Detect undocumented patterns: strong conventions in code not mentioned in rule files
    let undocumented = detect_undocumented_patterns(naming, rule_content_found);

    AgentRuleResult {
        rule_files_found: rule_files,
        validated,
        drifted,
        undocumented,
    }
}

/// Detect strong coding patterns not mentioned in any agent rule file.
fn detect_undocumented_patterns(
    naming: &NamingResult,
    has_rule_files: bool,
) -> Vec<UndocumentedPattern> {
    let mut patterns = Vec::new();

    // If there are no rule files at all, report the dominant naming convention
    if !has_rule_files && naming.adherence_rate > 0.7 {
        patterns.push(UndocumentedPattern {
            pattern: format!("{} naming", naming.dominant),
            description: format!(
                "Codebase uses {} at {:.0}% adherence but no agent rule file documents this",
                naming.dominant,
                naming.adherence_rate * 100.0
            ),
        });
    }

    // Report common prefixes used consistently (>5 occurrences)
    for (prefix, count) in &naming.common_prefixes {
        if *count >= 5 {
            patterns.push(UndocumentedPattern {
                pattern: format!("{prefix}* prefix convention"),
                description: format!("The prefix '{prefix}' is used in {count} identifiers"),
            });
        }
    }

    patterns
}

fn check_naming_rules(
    content: &str,
    source: &str,
    naming: &NamingResult,
    validated: &mut Vec<ValidatedRule>,
    drifted: &mut Vec<DriftedRule>,
) {
    let lower = content.to_lowercase();

    // Check for explicit snake_case mentions
    if lower.contains("snake_case") || lower.contains("snake case") {
        if naming.dominant == "snake_case" {
            validated.push(ValidatedRule {
                source: source.to_string(),
                rule: "Use snake_case naming".to_string(),
                actual: format!(
                    "Dominant: snake_case ({:.0}%)",
                    naming.adherence_rate * 100.0
                ),
            });
        } else {
            drifted.push(DriftedRule {
                source: source.to_string(),
                stated_convention: "snake_case".to_string(),
                actual_pattern: format!(
                    "Dominant: {} ({:.0}%)",
                    naming.dominant,
                    naming.adherence_rate * 100.0
                ),
                adherence_rate: naming
                    .conventions
                    .iter()
                    .find(|c| c.convention == "snake_case")
                    .map_or(0.0, |c| c.percentage / 100.0),
            });
        }
    }

    // Check for explicit camelCase mentions
    if lower.contains("camelcase") || lower.contains("camel case") {
        if naming.dominant == "camelCase" {
            validated.push(ValidatedRule {
                source: source.to_string(),
                rule: "Use camelCase naming".to_string(),
                actual: format!(
                    "Dominant: camelCase ({:.0}%)",
                    naming.adherence_rate * 100.0
                ),
            });
        } else {
            drifted.push(DriftedRule {
                source: source.to_string(),
                stated_convention: "camelCase".to_string(),
                actual_pattern: format!(
                    "Dominant: {} ({:.0}%)",
                    naming.dominant,
                    naming.adherence_rate * 100.0
                ),
                adherence_rate: naming
                    .conventions
                    .iter()
                    .find(|c| c.convention == "camelCase")
                    .map_or(0.0, |c| c.percentage / 100.0),
            });
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

async fn find_root_module(
    store: &dyn HomerStore,
) -> crate::error::Result<Option<crate::types::NodeId>> {
    let mod_filter = NodeFilter {
        kind: Some(NodeKind::Module),
        ..Default::default()
    };
    let modules = store.find_nodes(&mod_filter).await?;

    // Root module is typically "." or the shortest-named module
    let root = modules.iter().min_by_key(|m| m.name.len());

    Ok(root.map(|m| m.id))
}

async fn store_result<T: Serialize>(
    store: &dyn HomerStore,
    node_id: crate::types::NodeId,
    kind: AnalysisKind,
    data: &T,
) -> crate::error::Result<()> {
    let json = serde_json::to_value(data).map_err(|e| {
        crate::error::HomerError::Render(crate::error::RenderError::Template(e.to_string()))
    })?;

    store
        .store_analysis(&AnalysisResult {
            id: AnalysisResultId(0),
            node_id,
            kind,
            data: json,
            input_hash: 0,
            computed_at: Utc::now(),
        })
        .await?;

    Ok(())
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_snake_case() {
        assert_eq!(classify_name("hello_world"), "snake_case");
        assert_eq!(classify_name("get_user_by_id"), "snake_case");
    }

    #[test]
    fn classify_camel_case() {
        assert_eq!(classify_name("helloWorld"), "camelCase");
        assert_eq!(classify_name("getUserById"), "camelCase");
    }

    #[test]
    fn classify_pascal_case() {
        assert_eq!(classify_name("HelloWorld"), "PascalCase");
        assert_eq!(classify_name("UserService"), "PascalCase");
    }

    #[test]
    fn classify_screaming_snake() {
        assert_eq!(classify_name("MAX_SIZE"), "SCREAMING_SNAKE_CASE");
        assert_eq!(classify_name("HTTP_STATUS"), "SCREAMING_SNAKE_CASE");
    }

    #[test]
    fn classify_lowercase() {
        assert_eq!(classify_name("main"), "lowercase");
        assert_eq!(classify_name("run"), "lowercase");
    }

    #[test]
    fn leaf_name_from_qualified() {
        assert_eq!(leaf_name("src/main.rs::main"), "main");
        assert_eq!(leaf_name("src/lib.rs::UserService"), "UserService");
        assert_eq!(leaf_name("simple"), "simple");
    }

    #[test]
    fn detect_rust_error_patterns() {
        let source = "fn foo() -> Result<(), Error> { bar()? }";
        let patterns = detect_error_patterns(source, "rust");
        assert!(patterns.iter().any(|(p, _)| p == "? operator"));
        assert!(patterns.iter().any(|(p, _)| p == "Result<T, E>"));
    }

    #[test]
    fn detect_python_error_patterns() {
        let source = "try:\n    foo()\nexcept ValueError:\n    raise RuntimeError()";
        let patterns = detect_error_patterns(source, "python");
        assert!(patterns.iter().any(|(p, _)| p == "try/except"));
        assert!(patterns.iter().any(|(p, _)| p == "raise"));
    }

    #[test]
    fn detect_go_error_patterns() {
        let source = "if err != nil { return err }";
        let patterns = detect_error_patterns(source, "go");
        assert!(patterns.iter().any(|(p, _)| p == "if err != nil"));
    }

    #[test]
    fn source_file_detection() {
        assert!(is_source_file("src/main.rs"));
        assert!(is_source_file("app.ts"));
        assert!(is_source_file("utils.py"));
        assert!(!is_source_file("README.md"));
        assert!(!is_source_file("Cargo.toml"));
    }

    #[tokio::test]
    async fn full_convention_analysis() {
        use crate::extract::traits::Extractor;

        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("src")).unwrap();
        std::fs::write(
            tmp.path().join("src/main.rs"),
            "/// Entry point.\nfn main() {\n    get_user();\n}\n\nfn get_user() -> Result<(), String> {\n    Ok(())\n}\n",
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"test\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let db = crate::store::sqlite::SqliteStore::in_memory().unwrap();
        let config = HomerConfig::default();

        // Run structure + graph extraction first
        let struct_ext = crate::extract::structure::StructureExtractor::new(tmp.path());
        struct_ext.extract(&db, &config).await.unwrap();

        let graph_ext = crate::extract::graph::GraphExtractor::new(tmp.path());
        graph_ext.extract(&db, &config).await.unwrap();

        // Run convention analysis
        let analyzer = ConventionAnalyzer::new(tmp.path());
        let stats = analyzer.analyze(&db, &config).await.unwrap();

        assert_eq!(stats.results_stored, 5, "Should store 5 convention results");

        // Check naming result
        let mod_filter = NodeFilter {
            kind: Some(NodeKind::Module),
            ..Default::default()
        };
        let modules = db.find_nodes(&mod_filter).await.unwrap();
        let root = modules.iter().min_by_key(|m| m.name.len()).unwrap();

        let naming_result = db
            .get_analysis(root.id, AnalysisKind::NamingPattern)
            .await
            .unwrap();
        assert!(naming_result.is_some(), "Should have naming pattern result");

        let testing_result = db
            .get_analysis(root.id, AnalysisKind::TestingPattern)
            .await
            .unwrap();
        assert!(
            testing_result.is_some(),
            "Should have testing pattern result"
        );

        let test_data = testing_result.unwrap();
        let framework = test_data.data.get("framework").unwrap();
        assert_eq!(framework, "cargo test");
    }
}
