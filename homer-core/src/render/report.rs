// Report renderer — produces `homer-report.html` with project health dashboard.
//
// Sections: Executive Summary, Architecture Diagram, Hotspot Map,
// Coupling Analysis, Trend Charts, Risk Assessment, Documentation Health,
// Agent Effectiveness, Team Topology.

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::collections::HashMap;
use std::fmt::Write as _;

use tracing::{info, instrument};

use crate::config::HomerConfig;
use crate::store::HomerStore;
use crate::types::{AnalysisKind, NodeFilter, NodeId, NodeKind};

use super::traits::Renderer;

#[derive(Debug)]
pub struct ReportRenderer;

#[async_trait::async_trait]
impl Renderer for ReportRenderer {
    fn name(&self) -> &'static str {
        "report"
    }

    fn output_path(&self) -> &'static str {
        "homer-report.html"
    }

    #[instrument(skip_all, name = "report_render")]
    async fn render(
        &self,
        store: &dyn HomerStore,
        _config: &HomerConfig,
    ) -> crate::error::Result<String> {
        let data = load_report_data(store).await?;
        let out = render_html(&data);
        info!(bytes = out.len(), "Report rendered");
        Ok(out)
    }
}

// ── Report data model ────────────────────────────────────────────────

struct ReportData {
    file_count: u32,
    function_count: u32,
    type_count: u32,
    commit_count: u32,
    contributor_count: u32,
    community_count: u32,
    avg_bus_factor: f64,
    hotspots: Vec<HotspotEntry>,
    communities: HashMap<u32, Vec<String>>,
    coupling_pairs: Vec<(String, String, f64)>,
    risk_areas: Vec<RiskEntry>,
    documentation_coverage: f64,
    documented_entity_count: u32,
    total_entity_count: u32,
    trends: Vec<TrendEntry>,
    correction_hotspots: Vec<AgentEntry>,
    prompt_hotspots: Vec<AgentEntry>,
    contributor_distribution: Vec<ContributorEntry>,
}

struct HotspotEntry {
    name: String,
    score: f64,
    classification: String,
}

struct RiskEntry {
    path: String,
    score: f64,
    reasons: Vec<String>,
}

struct TrendEntry {
    name: String,
    trend: String,
}

struct AgentEntry {
    name: String,
    score: f64,
}

struct ContributorEntry {
    name: String,
    bus_factor: u32,
    top_contributor_pct: f64,
}

// ── Data loading ─────────────────────────────────────────────────────

async fn load_report_data(store: &dyn HomerStore) -> crate::error::Result<ReportData> {
    let file_count = count_nodes(store, NodeKind::File).await?;
    let function_count = count_nodes(store, NodeKind::Function).await?;
    let type_count = count_nodes(store, NodeKind::Type).await?;
    let commit_count = count_nodes(store, NodeKind::Commit).await?;
    let contributor_count = count_nodes(store, NodeKind::Contributor).await?;

    let hotspots = load_hotspots(store).await?;
    let (communities, community_count) = load_communities(store).await?;
    let avg_bus_factor = load_avg_bus_factor(store).await?;
    let coupling_pairs = load_coupling_pairs(store).await?;
    let risk_areas = load_risk_areas(store).await?;
    let (documentation_coverage, total_entity_count, documented_entity_count) =
        load_doc_coverage(store).await?;
    let trends = load_trends(store).await?;
    let correction_hotspots = load_agent_entries(store, AnalysisKind::CorrectionHotspot).await?;
    let prompt_hotspots = load_agent_entries(store, AnalysisKind::PromptHotspot).await?;
    let contributor_distribution = load_contributor_distribution(store).await?;

    Ok(ReportData {
        file_count,
        function_count,
        type_count,
        commit_count,
        contributor_count,
        community_count,
        avg_bus_factor,
        hotspots,
        communities,
        coupling_pairs,
        risk_areas,
        documentation_coverage,
        documented_entity_count,
        total_entity_count,
        trends,
        correction_hotspots,
        prompt_hotspots,
        contributor_distribution,
    })
}

async fn count_nodes(store: &dyn HomerStore, kind: NodeKind) -> crate::error::Result<u32> {
    let nodes = store
        .find_nodes(&NodeFilter {
            kind: Some(kind),
            ..Default::default()
        })
        .await?;
    Ok(nodes.len() as u32)
}

async fn load_hotspots(store: &dyn HomerStore) -> crate::error::Result<Vec<HotspotEntry>> {
    let results = store
        .get_analyses_by_kind(AnalysisKind::CompositeSalience)
        .await?;
    let mut hotspots: Vec<HotspotEntry> = Vec::new();
    for r in &results {
        let salience = r
            .data
            .get("score")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        let cls = r
            .data
            .get("classification")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Unknown");
        let name = resolve_name(store, r.node_id).await?;
        hotspots.push(HotspotEntry {
            name,
            score: salience,
            classification: cls.to_string(),
        });
    }
    hotspots.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hotspots.truncate(20);
    Ok(hotspots)
}

async fn load_communities(
    store: &dyn HomerStore,
) -> crate::error::Result<(HashMap<u32, Vec<String>>, u32)> {
    let results = store
        .get_analyses_by_kind(AnalysisKind::CommunityAssignment)
        .await?;
    let mut communities: HashMap<u32, Vec<String>> = HashMap::new();
    for r in &results {
        let cid = r
            .data
            .get("community_id")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32;
        let name = resolve_name(store, r.node_id).await?;
        communities.entry(cid).or_default().push(name);
    }
    let count = communities.len() as u32;
    Ok((communities, count))
}

async fn load_avg_bus_factor(store: &dyn HomerStore) -> crate::error::Result<f64> {
    let results = store
        .get_analyses_by_kind(AnalysisKind::ContributorConcentration)
        .await?;
    let values: Vec<f64> = results
        .iter()
        .filter_map(|r| {
            r.data
                .get("bus_factor")
                .and_then(serde_json::Value::as_u64)
                .map(|v| f64::from(v as u32))
        })
        .collect();
    if values.is_empty() {
        return Ok(0.0);
    }
    Ok(values.iter().sum::<f64>() / values.len() as f64)
}

async fn load_coupling_pairs(
    store: &dyn HomerStore,
) -> crate::error::Result<Vec<(String, String, f64)>> {
    let freq_results = store
        .get_analyses_by_kind(AnalysisKind::ChangeFrequency)
        .await?;
    let mut pairs: Vec<(String, String, f64)> = Vec::new();
    for result in &freq_results {
        let Some(partners) = result
            .data
            .get("co_change_partners")
            .and_then(|v| v.as_array())
        else {
            continue;
        };
        let source = resolve_name(store, result.node_id).await?;
        for p in partners.iter().take(2) {
            let target = p.get("file").and_then(|v| v.as_str()).unwrap_or("?");
            let conf = p
                .get("confidence")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.0);
            pairs.push((source.clone(), target.to_string(), conf));
        }
    }
    pairs.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    pairs.dedup_by(|a, b| (a.0 == b.0 && a.1 == b.1) || (a.0 == b.1 && a.1 == b.0));
    pairs.truncate(10);
    Ok(pairs)
}

async fn load_risk_areas(store: &dyn HomerStore) -> crate::error::Result<Vec<RiskEntry>> {
    let salience_results = store
        .get_analyses_by_kind(AnalysisKind::CompositeSalience)
        .await?;
    let stability_results = store
        .get_analyses_by_kind(AnalysisKind::StabilityClassification)
        .await?;
    let stability_map: HashMap<NodeId, String> = stability_results
        .iter()
        .filter_map(|r| {
            let cls = r
                .data
                .get("classification")
                .and_then(serde_json::Value::as_str)?;
            Some((r.node_id, cls.to_string()))
        })
        .collect();
    let bus_results = store
        .get_analyses_by_kind(AnalysisKind::ContributorConcentration)
        .await?;
    let bus_map: HashMap<NodeId, u64> = bus_results
        .iter()
        .filter_map(|r| {
            let bf = r.data.get("bus_factor")?.as_u64()?;
            Some((r.node_id, bf))
        })
        .collect();

    let mut areas: Vec<RiskEntry> = Vec::new();
    for r in &salience_results {
        let salience = r
            .data
            .get("score")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        let bf = bus_map.get(&r.node_id).copied().unwrap_or(u64::MAX);
        let mut reasons = Vec::new();

        if bf <= 1 {
            reasons.push("Low bus factor".to_string());
        }
        if stability_map
            .get(&r.node_id)
            .is_some_and(|s| s == "ActiveCritical")
        {
            reasons.push("Volatile + critical".to_string());
        }
        if reasons.is_empty() {
            continue;
        }

        let name = resolve_name(store, r.node_id).await?;
        areas.push(RiskEntry {
            path: name,
            score: salience,
            reasons,
        });
    }
    areas.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    areas.truncate(10);
    Ok(areas)
}

async fn load_doc_coverage(store: &dyn HomerStore) -> crate::error::Result<(f64, u32, u32)> {
    let results = store
        .get_analyses_by_kind(AnalysisKind::DocumentationStylePattern)
        .await?;
    Ok(results.first().map_or((0.0, 0, 0), |r| {
        let cov = r
            .data
            .get("coverage_rate")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        let total = r
            .data
            .get("total_entities")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32;
        let documented = r
            .data
            .get("documented_count")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32;
        (cov, total, documented)
    }))
}

async fn load_trends(store: &dyn HomerStore) -> crate::error::Result<Vec<TrendEntry>> {
    let results = store
        .get_analyses_by_kind(AnalysisKind::CentralityTrend)
        .await?;
    let mut trends: Vec<TrendEntry> = Vec::new();
    for r in &results {
        let trend = r
            .data
            .get("trend")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Stable");
        if trend != "Stable" {
            let name = resolve_name(store, r.node_id).await?;
            trends.push(TrendEntry {
                name,
                trend: trend.to_string(),
            });
        }
    }
    trends.truncate(10);
    Ok(trends)
}

async fn load_agent_entries(
    store: &dyn HomerStore,
    kind: AnalysisKind,
) -> crate::error::Result<Vec<AgentEntry>> {
    let results = store.get_analyses_by_kind(kind).await?;
    let mut entries: Vec<AgentEntry> = Vec::new();
    for r in &results {
        let val = r
            .data
            .get("score")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        if val > 0.0 {
            let name = resolve_name(store, r.node_id).await?;
            entries.push(AgentEntry { name, score: val });
        }
    }
    entries.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    entries.truncate(15);
    Ok(entries)
}

async fn load_contributor_distribution(
    store: &dyn HomerStore,
) -> crate::error::Result<Vec<ContributorEntry>> {
    let results = store
        .get_analyses_by_kind(AnalysisKind::ContributorConcentration)
        .await?;
    let mut entries: Vec<ContributorEntry> = Vec::new();
    for r in &results {
        let bus_factor = r
            .data
            .get("bus_factor")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32;
        let top_pct = r
            .data
            .get("top_contributor_pct")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        if bus_factor <= 1 || top_pct > 0.8 {
            let name = resolve_name(store, r.node_id).await?;
            entries.push(ContributorEntry {
                name,
                bus_factor,
                top_contributor_pct: top_pct,
            });
        }
    }
    entries.sort_by(|a, b| a.bus_factor.cmp(&b.bus_factor));
    entries.truncate(15);
    Ok(entries)
}

async fn resolve_name(store: &dyn HomerStore, node_id: NodeId) -> crate::error::Result<String> {
    Ok(store
        .get_node(node_id)
        .await?
        .map_or_else(|| format!("node:{}", node_id.0), |n| n.name))
}

// ── HTML rendering ───────────────────────────────────────────────────

fn render_html(data: &ReportData) -> String {
    let mut h = String::with_capacity(8192);

    let _ = writeln!(h, "<!DOCTYPE html>");
    let _ = writeln!(h, "<html lang=\"en\">");
    let _ = writeln!(h, "<head>");
    let _ = writeln!(h, "<meta charset=\"utf-8\">");
    let _ = writeln!(
        h,
        "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">"
    );
    let _ = writeln!(h, "<title>Homer Report</title>");
    let _ = writeln!(
        h,
        "<script src=\"https://cdn.jsdelivr.net/npm/mermaid/dist/mermaid.min.js\"></script>"
    );
    let _ = writeln!(h, "<style>{REPORT_CSS}</style>");
    let _ = writeln!(h, "</head>");
    let _ = writeln!(h, "<body>");
    let _ = writeln!(
        h,
        "<script>mermaid.initialize({{startOnLoad:true}});</script>"
    );

    render_executive_summary(&mut h, data);
    render_architecture_diagram(&mut h, data);
    render_hotspot_map(&mut h, data);
    render_coupling_section(&mut h, data);
    render_trends_section(&mut h, data);
    render_risk_section(&mut h, data);
    render_doc_health(&mut h, data);
    render_agent_effectiveness(&mut h, data);
    render_team_topology(&mut h, data);

    let _ = writeln!(h, "<footer><p>Generated by Homer</p></footer>");
    let _ = writeln!(h, "</body>");
    let _ = writeln!(h, "</html>");

    h
}

fn render_executive_summary(h: &mut String, data: &ReportData) {
    let _ = writeln!(h, "<h1>Homer Report</h1>");
    let _ = writeln!(h, "<section class=\"summary\">");
    let _ = writeln!(h, "<h2>Executive Summary</h2>");
    let _ = writeln!(h, "<div class=\"metrics\">");

    emit_metric(h, "Files", &data.file_count.to_string());
    emit_metric(h, "Functions", &data.function_count.to_string());
    emit_metric(h, "Types", &data.type_count.to_string());
    emit_metric(h, "Commits", &data.commit_count.to_string());
    emit_metric(h, "Contributors", &data.contributor_count.to_string());
    emit_metric(h, "Communities", &data.community_count.to_string());
    emit_metric(h, "Avg Bus Factor", &format!("{:.1}", data.avg_bus_factor));
    emit_metric(
        h,
        "Doc Coverage",
        &format!("{:.0}%", data.documentation_coverage * 100.0),
    );

    let _ = writeln!(h, "</div>");
    let _ = writeln!(h, "</section>");
}

fn emit_metric(h: &mut String, label: &str, value: &str) {
    let _ = writeln!(
        h,
        "<div class=\"metric\"><span class=\"value\">{value}</span>\
         <span class=\"label\">{label}</span></div>"
    );
}

fn render_architecture_diagram(h: &mut String, data: &ReportData) {
    if data.communities.is_empty() {
        return;
    }

    let _ = writeln!(h, "<section>");
    let _ = writeln!(h, "<h2>Architecture Diagram</h2>");
    let _ = writeln!(h, "<div class=\"mermaid\">");
    let _ = writeln!(h, "graph TD");

    let mut sorted: Vec<_> = data.communities.iter().collect();
    sorted.sort_by_key(|(cid, _)| *cid);

    for (cid, members) in &sorted {
        let dir_label = members.first().map_or("cluster", |m| {
            m.rfind('/').map_or(m.as_str(), |idx| &m[..idx])
        });
        let _ = writeln!(h, "  subgraph C{cid}[\"{dir_label}\"]");
        for (idx, member) in members.iter().take(5).enumerate() {
            let short = member.rsplit('/').next().unwrap_or(member);
            let _ = writeln!(h, "    N{cid}_{idx}[\"{short}\"]");
        }
        if members.len() > 5 {
            let _ = writeln!(h, "    N{cid}_more[\"...+{}\"]", members.len() - 5);
        }
        let _ = writeln!(h, "  end");
    }

    let _ = writeln!(h, "</div>");
    let _ = writeln!(h, "</section>");
}

fn render_hotspot_map(h: &mut String, data: &ReportData) {
    if data.hotspots.is_empty() {
        return;
    }

    let _ = writeln!(h, "<section>");
    let _ = writeln!(h, "<h2>Hotspot Map</h2>");
    let _ = writeln!(h, "<p>Top entities by composite salience score.</p>");
    let _ = writeln!(
        h,
        "<table><thead><tr><th>Entity</th><th>Salience</th>\
         <th>Classification</th><th>Bar</th></tr></thead><tbody>"
    );

    for entry in &data.hotspots {
        let short = entry.name.rsplit("::").next().unwrap_or(&entry.name);
        let bar_px = (entry.score * 200.0) as u32;
        let color = salience_color(entry.score);
        let _ = writeln!(
            h,
            "<tr><td><code>{short}</code></td><td>{:.2}</td><td>{}</td>\
             <td><div class=\"bar\" style=\"width:{bar_px}px;background:{color}\"></div></td></tr>",
            entry.score, entry.classification
        );
    }

    let _ = writeln!(h, "</tbody></table>");
    let _ = writeln!(h, "</section>");
}

fn render_coupling_section(h: &mut String, data: &ReportData) {
    if data.coupling_pairs.is_empty() {
        return;
    }

    let _ = writeln!(h, "<section>");
    let _ = writeln!(h, "<h2>Coupling Analysis</h2>");
    let _ = writeln!(h, "<p>Files that frequently change together.</p>");
    let _ = writeln!(
        h,
        "<table><thead><tr><th>File A</th><th>File B</th>\
         <th>Confidence</th></tr></thead><tbody>"
    );

    for (file_a, file_b, conf) in &data.coupling_pairs {
        let sa = file_a.rsplit('/').next().unwrap_or(file_a);
        let sb = file_b.rsplit('/').next().unwrap_or(file_b);
        let _ = writeln!(
            h,
            "<tr><td><code>{sa}</code></td><td><code>{sb}</code></td>\
             <td>{conf:.0}%</td></tr>"
        );
    }

    let _ = writeln!(h, "</tbody></table>");
    let _ = writeln!(h, "</section>");
}

fn render_trends_section(h: &mut String, data: &ReportData) {
    if data.trends.is_empty() {
        return;
    }

    let _ = writeln!(h, "<section>");
    let _ = writeln!(h, "<h2>Centrality Trends</h2>");
    let _ = writeln!(h, "<p>Entities with changing structural importance.</p>");
    let _ = writeln!(
        h,
        "<table><thead><tr><th>Entity</th><th>Trend</th></tr></thead><tbody>"
    );

    for entry in &data.trends {
        let short = entry.name.rsplit("::").next().unwrap_or(&entry.name);
        let icon = match entry.trend.as_str() {
            "Rising" => "&#x2191;",
            "Falling" => "&#x2193;",
            _ => "&#x2194;",
        };
        let _ = writeln!(
            h,
            "<tr><td><code>{short}</code></td><td>{icon} {}</td></tr>",
            entry.trend
        );
    }

    let _ = writeln!(h, "</tbody></table>");
    let _ = writeln!(h, "</section>");
}

fn render_risk_section(h: &mut String, data: &ReportData) {
    if data.risk_areas.is_empty() {
        return;
    }

    let _ = writeln!(h, "<section>");
    let _ = writeln!(h, "<h2>Risk Assessment</h2>");
    let _ = writeln!(
        h,
        "<table><thead><tr><th>Area</th><th>Score</th>\
         <th>Reasons</th></tr></thead><tbody>"
    );

    for entry in &data.risk_areas {
        let short = entry.path.rsplit('/').next().unwrap_or(&entry.path);
        let _ = writeln!(
            h,
            "<tr><td><code>{short}</code></td><td>{:.2}</td><td>{}</td></tr>",
            entry.score,
            entry.reasons.join(", ")
        );
    }

    let _ = writeln!(h, "</tbody></table>");
    let _ = writeln!(h, "</section>");
}

fn render_doc_health(h: &mut String, data: &ReportData) {
    let _ = writeln!(h, "<section>");
    let _ = writeln!(h, "<h2>Documentation Health</h2>");
    let _ = writeln!(h, "<div class=\"metrics\">");
    emit_metric(
        h,
        "Coverage",
        &format!("{:.0}%", data.documentation_coverage * 100.0),
    );
    emit_metric(h, "Documented", &data.documented_entity_count.to_string());
    emit_metric(h, "Total Entities", &data.total_entity_count.to_string());
    let _ = writeln!(h, "</div>");
    let _ = writeln!(h, "</section>");
}

fn render_agent_effectiveness(h: &mut String, data: &ReportData) {
    if data.correction_hotspots.is_empty() && data.prompt_hotspots.is_empty() {
        return;
    }
    let _ = writeln!(h, "<section>");
    let _ = writeln!(h, "<h2>Agent Effectiveness</h2>");

    if !data.correction_hotspots.is_empty() {
        let _ = writeln!(h, "<h3>Correction Hotspots</h3>");
        let _ = writeln!(h, "<p>Files where agent corrections are most frequent:</p>");
        let _ = writeln!(h, "<table><tr><th>Entity</th><th>Score</th></tr>");
        for e in &data.correction_hotspots {
            let _ = writeln!(
                h,
                "<tr><td><code>{}</code></td><td>{:.2}</td></tr>",
                e.name, e.score
            );
        }
        let _ = writeln!(h, "</table>");
    }

    if !data.prompt_hotspots.is_empty() {
        let _ = writeln!(h, "<h3>Prompt Hotspots</h3>");
        let _ = writeln!(
            h,
            "<p>Files most frequently referenced in agent prompts:</p>"
        );
        let _ = writeln!(h, "<table><tr><th>Entity</th><th>Score</th></tr>");
        for e in &data.prompt_hotspots {
            let _ = writeln!(
                h,
                "<tr><td><code>{}</code></td><td>{:.2}</td></tr>",
                e.name, e.score
            );
        }
        let _ = writeln!(h, "</table>");
    }

    let _ = writeln!(h, "</section>");
}

fn render_team_topology(h: &mut String, data: &ReportData) {
    if data.contributor_distribution.is_empty() {
        return;
    }
    let _ = writeln!(h, "<section>");
    let _ = writeln!(h, "<h2>Team Topology</h2>");
    let _ = writeln!(
        h,
        "<p>Knowledge concentration risks (bus factor &le; 1 or single contributor &gt; 80%):</p>"
    );
    let _ = writeln!(
        h,
        "<table><tr><th>Entity</th><th>Bus Factor</th><th>Top Contributor %</th></tr>"
    );
    for e in &data.contributor_distribution {
        let _ = writeln!(
            h,
            "<tr><td><code>{}</code></td><td>{}</td><td>{:.0}%</td></tr>",
            e.name,
            e.bus_factor,
            e.top_contributor_pct * 100.0
        );
    }
    let _ = writeln!(h, "</table>");
    let _ = writeln!(h, "</section>");
}

fn salience_color(score: f64) -> &'static str {
    if score >= 0.8 {
        "#e74c3c"
    } else if score >= 0.6 {
        "#e67e22"
    } else if score >= 0.4 {
        "#f1c40f"
    } else if score >= 0.2 {
        "#2ecc71"
    } else {
        "#95a5a6"
    }
}

const REPORT_CSS: &str = "\
body{font-family:-apple-system,BlinkMacSystemFont,\"Segoe UI\",sans-serif;\
max-width:960px;margin:0 auto;padding:2rem;color:#333;background:#fafafa}\
h1{border-bottom:2px solid #333;padding-bottom:.5rem}\
h2{margin-top:2rem;color:#2c3e50}\
.summary{background:#fff;padding:1.5rem;border-radius:8px;\
box-shadow:0 1px 3px rgba(0,0,0,.1)}\
.metrics{display:flex;flex-wrap:wrap;gap:1rem}\
.metric{text-align:center;padding:1rem;background:#f8f9fa;\
border-radius:6px;min-width:100px}\
.metric .value{display:block;font-size:1.8rem;font-weight:bold;color:#2c3e50}\
.metric .label{font-size:.85rem;color:#7f8c8d}\
table{border-collapse:collapse;width:100%;margin:1rem 0}\
th,td{padding:.5rem .75rem;text-align:left;border-bottom:1px solid #e0e0e0}\
th{background:#f8f9fa;font-weight:600}\
code{background:#f0f0f0;padding:2px 6px;border-radius:3px;font-size:.9em}\
.bar{height:16px;border-radius:3px}\
section{margin-bottom:2rem}\
footer{margin-top:3rem;text-align:center;color:#aaa;font-size:.85rem}\
.mermaid{background:#fff;padding:1rem;border-radius:8px}";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::sqlite::SqliteStore;
    use crate::types::{AnalysisResult, AnalysisResultId, Node, NodeId};
    use chrono::Utc;

    #[tokio::test]
    async fn renders_html_report() {
        let store = SqliteStore::in_memory().unwrap();
        let now = Utc::now();

        let file_id = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::File,
                name: "src/main.rs".to_string(),
                content_hash: None,
                last_extracted: now,
                metadata: HashMap::new(),
            })
            .await
            .unwrap();

        store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::Function,
                name: "src/main.rs::main".to_string(),
                content_hash: None,
                last_extracted: now,
                metadata: HashMap::new(),
            })
            .await
            .unwrap();

        store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::Contributor,
                name: "dev@test.com".to_string(),
                content_hash: None,
                last_extracted: now,
                metadata: HashMap::new(),
            })
            .await
            .unwrap();

        store
            .store_analysis(&AnalysisResult {
                id: AnalysisResultId(0),
                node_id: file_id,
                kind: AnalysisKind::CompositeSalience,
                data: serde_json::json!({
                    "score": 0.85,
                    "classification": "HotCritical",
                    "components": { "pagerank": 0.9 }
                }),
                input_hash: 0,
                computed_at: now,
            })
            .await
            .unwrap();

        store
            .store_analysis(&AnalysisResult {
                id: AnalysisResultId(0),
                node_id: file_id,
                kind: AnalysisKind::CommunityAssignment,
                data: serde_json::json!({ "community_id": 0 }),
                input_hash: 0,
                computed_at: now,
            })
            .await
            .unwrap();

        let config = HomerConfig::default();
        let renderer = ReportRenderer;
        let output = renderer.render(&store, &config).await.unwrap();

        assert!(output.contains("<!DOCTYPE html>"), "Should be valid HTML");
        assert!(output.contains("Homer Report"), "Should have title");
        assert!(output.contains("Executive Summary"), "Should have summary");
        assert!(output.contains("Hotspot Map"), "Should have hotspot map");
        assert!(output.contains("main.rs"), "Should mention main.rs");
        assert!(
            output.contains("Architecture Diagram"),
            "Should have architecture diagram"
        );
    }

    #[tokio::test]
    async fn empty_store_produces_minimal_report() {
        let store = SqliteStore::in_memory().unwrap();
        let config = HomerConfig::default();
        let renderer = ReportRenderer;
        let output = renderer.render(&store, &config).await.unwrap();

        assert!(output.contains("<!DOCTYPE html>"), "Should be valid HTML");
        assert!(
            output.contains("Executive Summary"),
            "Should have summary even with no data"
        );
        assert!(
            output.contains("Documentation Health"),
            "Should have doc health section"
        );
    }
}
