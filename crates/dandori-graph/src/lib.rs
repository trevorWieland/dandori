use std::collections::BTreeSet;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct EntityId(pub uuid::Uuid);

impl EntityId {
    #[must_use]
    pub fn new() -> Self {
        Self(uuid::Uuid::now_v7())
    }
}

impl Default for EntityId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GraphEdge {
    pub from: EntityId,
    pub to: EntityId,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GraphSpec {
    pub nodes: Vec<EntityId>,
    pub edges: Vec<GraphEdge>,
}

impl GraphSpec {
    pub fn validate(&self) -> Result<(), CrateError> {
        if self.nodes.is_empty() {
            return Err(CrateError::Validation(
                "graph must contain at least one node".to_owned(),
            ));
        }

        let node_set: BTreeSet<EntityId> = self.nodes.iter().copied().collect();
        let mut seen_edges = BTreeSet::new();

        for edge in &self.edges {
            if edge.from == edge.to {
                return Err(CrateError::Validation(
                    "self-loop edges are not allowed".to_owned(),
                ));
            }
            if !node_set.contains(&edge.from) || !node_set.contains(&edge.to) {
                return Err(CrateError::Validation(
                    "edges must reference known nodes".to_owned(),
                ));
            }
            if !seen_edges.insert((edge.from, edge.to)) {
                return Err(CrateError::Validation(
                    "duplicate directed edges are not allowed".to_owned(),
                ));
            }
        }

        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CrateError {
    #[error("validation error: {0}")]
    Validation(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_spec_rejects_duplicate_edges() {
        let a = EntityId::new();
        let b = EntityId::new();
        let spec = GraphSpec {
            nodes: vec![a, b],
            edges: vec![GraphEdge { from: a, to: b }, GraphEdge { from: a, to: b }],
        };

        let error = spec
            .validate()
            .expect_err("duplicate edges must fail validation");
        assert!(matches!(error, CrateError::Validation(_)));
    }
}
