//! API change analysis and committed base-graph loading.

mod base;
mod diff;

pub use base::{load_base_graph, BaseGraph};
pub use diff::{
    diff_graphs, diff_graphs_with_gate_operations, AffectedOperation, Change, ChangeKind,
    ChangePolicy, ChangeReport, ChangeSummary, Sides,
};
