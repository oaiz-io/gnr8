//! API change analysis and committed base-graph loading.

mod acceptance;
mod base;
mod diff;

pub use acceptance::{
    apply_change_acceptances, load_change_acceptances, AcceptanceError, AcceptedChange,
    ChangeAcceptance, ChangeAcceptances, ACCEPTANCE_SCHEMA_VERSION, DEFAULT_ACCEPTANCE_PATH,
};
pub use base::{load_base_graph, BaseGraph};
pub use diff::{
    diff_graphs_with_gate_operations, AffectedOperation, Change, ChangeKind, ChangePolicy,
    ChangeReport, ChangeSummary, GateOperation, GateOperationParseError, Sides,
    GATE_OPERATION_SHAPE,
};
