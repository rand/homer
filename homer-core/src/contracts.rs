//! Canonical cross-component contracts for edge roles and analysis/metadata keys.
//!
//! This module centralizes role and key strings that are shared between
//! extractors, analyzers, renderers, CLI, and MCP integration points.

use crate::store::HomerStore;
use crate::types::{HyperedgeMember, NodeFilter, NodeId, NodeKind};

/// Canonical hyperedge member roles and legacy aliases.
pub mod roles {
    /// Calls edge roles.
    pub const CALLER: &str = "caller";
    pub const CALLEE: &str = "callee";

    /// Imports edge roles.
    pub const IMPORTER: &str = "importer";
    pub const IMPORTED: &str = "imported";
    pub const IMPORTER_LEGACY: &str = "source";
    pub const IMPORTED_LEGACY: &str = "target";

    /// Documents edge roles.
    pub const DOCUMENT: &str = "document";
    pub const CODE_ENTITY: &str = "code_entity";
    pub const CODE_ENTITY_LEGACY_A: &str = "entity";
    pub const CODE_ENTITY_LEGACY_B: &str = "subject";

    /// Generic containment roles.
    pub const MEMBER: &str = "member";
    pub const CONTAINER: &str = "container";
}

/// Canonical analysis result keys and legacy aliases.
pub mod analysis_keys {
    pub const SCORE: &str = "score";
    pub const PAGERANK: &str = "pagerank";
    pub const BETWEENNESS: &str = "betweenness";
    pub const AUTHORITY: &str = "authority";
    pub const AUTHORITY_SCORE: &str = "authority_score";
    pub const BUS_FACTOR: &str = "bus_factor";
    pub const TOP_CONTRIBUTOR_SHARE: &str = "top_contributor_share";
    pub const TOP_CONTRIBUTOR_SHARE_LEGACY: &str = "top_contributor_pct";
}

/// Canonical node metadata keys.
pub mod metadata_keys {
    pub const DOC_TYPE: &str = "doc_type";
}

/// Return the first member whose role matches one of `roles`.
pub fn find_member_by_roles<'a>(
    members: &'a [HyperedgeMember],
    roles: &[&str],
) -> Option<&'a HyperedgeMember> {
    roles
        .iter()
        .find_map(|role| members.iter().find(|m| m.role == *role))
}

/// Resolve importer/imported members with legacy role compatibility.
pub fn find_import_pair(
    members: &[HyperedgeMember],
) -> Option<(&HyperedgeMember, &HyperedgeMember)> {
    let source_member = find_member_by_roles(members, &[roles::IMPORTER, roles::IMPORTER_LEGACY])?;
    let target_member = find_member_by_roles(members, &[roles::IMPORTED, roles::IMPORTED_LEGACY])?;
    Some((source_member, target_member))
}

/// Resolve document/code-entity members with legacy role compatibility.
pub fn find_document_pair(
    members: &[HyperedgeMember],
) -> Option<(&HyperedgeMember, &HyperedgeMember)> {
    let document = find_member_by_roles(members, &[roles::DOCUMENT])?;
    let entity = find_member_by_roles(
        members,
        &[
            roles::CODE_ENTITY,
            roles::CODE_ENTITY_LEGACY_A,
            roles::CODE_ENTITY_LEGACY_B,
        ],
    )?;
    Some((document, entity))
}

/// Convert a 0..1 fraction to a 0..100 percentage value.
pub fn fraction_to_percent(value: f64) -> f64 {
    value * 100.0
}

/// Read top-contributor share with backward compatibility.
pub fn read_top_contributor_share(data: &serde_json::Value) -> Option<f64> {
    data.get(analysis_keys::TOP_CONTRIBUTOR_SHARE)
        .and_then(serde_json::Value::as_f64)
        .or_else(|| {
            data.get(analysis_keys::TOP_CONTRIBUTOR_SHARE_LEGACY)
                .and_then(serde_json::Value::as_f64)
        })
}

/// Find the repository root `Module` node.
///
/// Preferred order:
/// 1. Module with `metadata.is_root == true`
/// 2. Module named `.`
/// 3. Legacy fallback: shortest module name
pub async fn find_root_module_id(store: &dyn HomerStore) -> crate::error::Result<Option<NodeId>> {
    let modules = store
        .find_nodes(&NodeFilter {
            kind: Some(NodeKind::Module),
            ..Default::default()
        })
        .await?;
    if modules.is_empty() {
        return Ok(None);
    }

    if let Some(root) = modules.iter().find(|m| {
        m.metadata
            .get("is_root")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    }) {
        return Ok(Some(root.id));
    }

    if let Some(root) = modules.iter().find(|m| m.name == ".") {
        return Ok(Some(root.id));
    }

    Ok(modules.iter().min_by_key(|m| m.name.len()).map(|m| m.id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::sqlite::SqliteStore;
    use crate::types::{Node, NodeKind};
    use chrono::Utc;
    use std::collections::HashMap;

    #[test]
    fn import_pair_supports_canonical_and_legacy_roles() {
        let canonical = vec![
            HyperedgeMember {
                node_id: NodeId(1),
                role: roles::IMPORTER.to_string(),
                position: 0,
            },
            HyperedgeMember {
                node_id: NodeId(2),
                role: roles::IMPORTED.to_string(),
                position: 1,
            },
        ];
        let legacy = vec![
            HyperedgeMember {
                node_id: NodeId(3),
                role: roles::IMPORTER_LEGACY.to_string(),
                position: 0,
            },
            HyperedgeMember {
                node_id: NodeId(4),
                role: roles::IMPORTED_LEGACY.to_string(),
                position: 1,
            },
        ];

        let (src1, dst1) = find_import_pair(&canonical).expect("canonical roles should resolve");
        let (src2, dst2) = find_import_pair(&legacy).expect("legacy roles should resolve");

        assert_eq!(src1.node_id, NodeId(1));
        assert_eq!(dst1.node_id, NodeId(2));
        assert_eq!(src2.node_id, NodeId(3));
        assert_eq!(dst2.node_id, NodeId(4));
    }

    #[test]
    fn document_pair_supports_canonical_and_legacy_roles() {
        for entity_role in [
            roles::CODE_ENTITY,
            roles::CODE_ENTITY_LEGACY_A,
            roles::CODE_ENTITY_LEGACY_B,
        ] {
            let members = vec![
                HyperedgeMember {
                    node_id: NodeId(10),
                    role: roles::DOCUMENT.to_string(),
                    position: 0,
                },
                HyperedgeMember {
                    node_id: NodeId(11),
                    role: entity_role.to_string(),
                    position: 1,
                },
            ];
            let (doc, entity) =
                find_document_pair(&members).expect("document roles should resolve");
            assert_eq!(doc.node_id, NodeId(10));
            assert_eq!(entity.node_id, NodeId(11));
        }
    }

    #[test]
    fn top_contributor_share_supports_legacy_key() {
        let canonical = serde_json::json!({ analysis_keys::TOP_CONTRIBUTOR_SHARE: 0.91 });
        let legacy = serde_json::json!({ analysis_keys::TOP_CONTRIBUTOR_SHARE_LEGACY: 0.77 });

        assert_eq!(read_top_contributor_share(&canonical), Some(0.91));
        assert_eq!(read_top_contributor_share(&legacy), Some(0.77));
    }

    #[tokio::test]
    async fn root_module_prefers_is_root_metadata() {
        let store = SqliteStore::in_memory().expect("in-memory store");
        let now = Utc::now();

        let mut root_meta = HashMap::new();
        root_meta.insert("is_root".to_string(), serde_json::json!(true));

        let root_id = store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::Module,
                name: "repo-name".to_string(),
                content_hash: None,
                last_extracted: now,
                metadata: root_meta,
            })
            .await
            .expect("insert root");

        store
            .upsert_node(&Node {
                id: NodeId(0),
                kind: NodeKind::Module,
                name: "src".to_string(),
                content_hash: None,
                last_extracted: now,
                metadata: HashMap::new(),
            })
            .await
            .expect("insert child");

        let found = find_root_module_id(&store)
            .await
            .expect("find root")
            .expect("root exists");
        assert_eq!(found, root_id);
    }
}
