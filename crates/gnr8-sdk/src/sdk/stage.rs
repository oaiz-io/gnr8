//! Pipeline stages: built-in declarations, custom Rust, and the plan the host reads.
//!
//! A [`Pipeline`] stores each kind of stage in one ordered vector whose elements are either a
//! **built-in declaration** — serializable configuration the installed host executes — or a
//! **custom stage**, your own Rust wrapped in [`Custom`], executed in the worker process against
//! the graph the host sends over.
//!
//! [`StagePlan`] is the serializable description of that composition. It is the first thing the
//! worker sends to the host, and it is what lets the host run the whole pipeline in order while
//! calling back only for the stages it cannot run itself.

use crate::sdk::builtins;
use crate::sdk::{Pipeline, PostProcess, ReadinessTarget, Source, Target, Transform};

/// Wrap your own [`Source`]/[`Transform`]/[`Target`]/[`PostProcess`] so a pipeline can hold it.
///
/// ```no_run
/// # use gnr8::sdk::prelude::*;
/// # use gnr8::graph::ApiGraph;
/// # use gnr8::Error;
/// # struct DropDebugRoutes;
/// # impl Transform for DropDebugRoutes {
/// #     fn apply(&self, _ir: &mut ApiGraph, _cx: &Cx) -> Result<(), Error> { Ok(()) }
/// # }
/// Pipeline::new().transform(Custom(DropDebugRoutes));
/// ```
///
/// The wrapper is not ceremony for its own sake: it is the one place a reader can see which side of
/// the host/worker boundary a stage runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Custom<T>(pub T);

macro_rules! builtin_enum {
    (
        $(#[$meta:meta])*
        $name:ident { $($variant:ident),* $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
        #[serde(tag = "stage", content = "config", rename_all = "snake_case")]
        pub enum $name {
            $(
                /// The built-in stage of the same name.
                $variant(builtins::$variant),
            )*
        }

        impl $name {
            /// The stage's declaration name, used in host diagnostics and producer labels.
            #[must_use]
            pub fn label(&self) -> &'static str {
                match self {
                    $(Self::$variant(_) => stringify!($variant),)*
                }
            }
        }
    };
}

builtin_enum! {
    /// Every built-in source, as a declaration the host executes.
    BuiltinSource { GoGin, OpenApi, FastApi, Flask, NestJs }
}

builtin_enum! {
    /// Every built-in transform, as a declaration the host executes.
    BuiltinTransform {
        SetBasePath,
        SetTitle,
        OpenApiMetadata,
        DiagnosticPolicy,
        RequireOperationDocs,
        SetOperationSuccessResponse,
        SetSchemaFieldType,
        ApiOverrides,
        SetEnumOrder,
        ApplySecurity,
        ConfigureSdkRuntime,
        MarkIdempotent,
        ConfigurePagination,
        DocumentOperation,
        RenameOperation,
        RenameType,
        GroupOperations,
    }
}

builtin_enum! {
    /// Every built-in target, as a declaration the host executes.
    BuiltinTarget { OpenApi31, OpenApi31Json, StaticFiles, GoSdk, PySdk, TsSdk }
}

builtin_enum! {
    /// Every built-in post-processor, as a declaration the host executes.
    BuiltinPost { FormatCommand, Header }
}

/// One source stage: a built-in declaration or your own [`Source`].
///
/// A declaration is inline configuration data rather than a pointer: a pipeline holds a handful of
/// stages, and boxing every built-in to even out the variant sizes would trade one allocation per
/// stage for nothing measurable.
#[allow(clippy::large_enum_variant)]
pub enum SourceStage {
    /// A built-in source the host executes.
    Builtin(BuiltinSource),
    /// A user-authored source the worker executes.
    Custom(Box<dyn Source>),
}

/// One transform stage: a built-in declaration or your own [`Transform`].
#[allow(clippy::large_enum_variant)]
pub enum TransformStage {
    /// A built-in transform the host executes.
    Builtin(BuiltinTransform),
    /// A user-authored transform the worker executes.
    Custom(Box<dyn Transform>),
}

/// One target stage: a built-in declaration or your own [`Target`].
#[allow(clippy::large_enum_variant)]
pub enum TargetStage {
    /// A built-in target the host executes.
    Builtin(BuiltinTarget),
    /// A user-authored target the worker executes.
    Custom(Box<dyn Target>),
}

/// One post-process stage: a built-in declaration or your own [`PostProcess`].
#[allow(clippy::large_enum_variant)]
pub enum PostStage {
    /// A built-in post-processor the host executes.
    Builtin(BuiltinPost),
    /// A user-authored post-processor the worker executes.
    Custom(Box<dyn PostProcess>),
}

macro_rules! from_builtins {
    ($stage:ident, $builtin:ident, [$($variant:ident),* $(,)?]) => {
        $(
            impl From<builtins::$variant> for $stage {
                fn from(value: builtins::$variant) -> Self {
                    Self::Builtin($builtin::$variant(value))
                }
            }
        )*
    };
}

from_builtins!(
    SourceStage,
    BuiltinSource,
    [GoGin, OpenApi, FastApi, Flask, NestJs]
);
from_builtins!(
    TransformStage,
    BuiltinTransform,
    [
        SetBasePath,
        SetTitle,
        OpenApiMetadata,
        DiagnosticPolicy,
        RequireOperationDocs,
        SetOperationSuccessResponse,
        SetSchemaFieldType,
        ApiOverrides,
        SetEnumOrder,
        ApplySecurity,
        ConfigureSdkRuntime,
        MarkIdempotent,
        ConfigurePagination,
        DocumentOperation,
        RenameOperation,
        RenameType,
        GroupOperations,
    ]
);
from_builtins!(
    TargetStage,
    BuiltinTarget,
    [OpenApi31, OpenApi31Json, StaticFiles, GoSdk, PySdk, TsSdk]
);
from_builtins!(PostStage, BuiltinPost, [FormatCommand, Header]);

impl<T: Source + 'static> From<Custom<T>> for SourceStage {
    fn from(value: Custom<T>) -> Self {
        Self::Custom(Box::new(value.0))
    }
}

impl<T: Transform + 'static> From<Custom<T>> for TransformStage {
    fn from(value: Custom<T>) -> Self {
        Self::Custom(Box::new(value.0))
    }
}

impl<T: Target + 'static> From<Custom<T>> for TargetStage {
    fn from(value: Custom<T>) -> Self {
        Self::Custom(Box::new(value.0))
    }
}

impl<T: PostProcess + 'static> From<Custom<T>> for PostStage {
    fn from(value: Custom<T>) -> Self {
        Self::Custom(Box::new(value.0))
    }
}

/// One entry in a [`StagePlan`]: a built-in declaration, or the position and declarations of a
/// custom stage the host must call back for.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStage<B> {
    /// A built-in stage the host runs itself.
    Builtin(B),
    /// A user-authored stage the host asks the worker to run.
    Custom {
        /// Position of this stage within its kind's ordered vector.
        index: usize,
        /// The stage's Rust type name, used for producer labels and diagnostics.
        label: String,
        /// Loop-safety anchors declared by a custom target (empty for other stage kinds).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        output_anchors: Vec<String>,
        /// Readiness checks declared by a custom target (empty for other stage kinds).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        readiness_targets: Vec<ReadinessTarget>,
    },
}

/// The serializable description of a composed pipeline.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct StagePlan {
    /// Source stages, in composition order.
    pub sources: Vec<PlanStage<BuiltinSource>>,
    /// Transform stages, in composition order.
    pub transforms: Vec<PlanStage<BuiltinTransform>>,
    /// Target stages, in composition order.
    pub targets: Vec<PlanStage<BuiltinTarget>>,
    /// Post-process stages, in composition order.
    pub posts: Vec<PlanStage<BuiltinPost>>,
}

impl StagePlan {
    /// Describe `pipeline`.
    #[must_use]
    pub fn of(pipeline: &Pipeline) -> Self {
        Self {
            sources: pipeline
                .sources
                .iter()
                .enumerate()
                .map(|(index, stage)| match stage {
                    SourceStage::Builtin(spec) => PlanStage::Builtin(spec.clone()),
                    SourceStage::Custom(_) => PlanStage::Custom {
                        index,
                        label: "Source".to_string(),
                        output_anchors: Vec::new(),
                        readiness_targets: Vec::new(),
                    },
                })
                .collect(),
            transforms: pipeline
                .transforms
                .iter()
                .enumerate()
                .map(|(index, stage)| match stage {
                    TransformStage::Builtin(spec) => PlanStage::Builtin(spec.clone()),
                    TransformStage::Custom(_) => PlanStage::Custom {
                        index,
                        label: "Transform".to_string(),
                        output_anchors: Vec::new(),
                        readiness_targets: Vec::new(),
                    },
                })
                .collect(),
            targets: pipeline
                .targets
                .iter()
                .enumerate()
                .map(|(index, stage)| match stage {
                    TargetStage::Builtin(spec) => PlanStage::Builtin(spec.clone()),
                    TargetStage::Custom(target) => PlanStage::Custom {
                        index,
                        label: target.producer().to_string(),
                        output_anchors: target.output_anchors(),
                        readiness_targets: target.readiness_targets(),
                    },
                })
                .collect(),
            posts: pipeline
                .posts
                .iter()
                .enumerate()
                .map(|(index, stage)| match stage {
                    PostStage::Builtin(spec) => PlanStage::Builtin(spec.clone()),
                    PostStage::Custom(post) => PlanStage::Custom {
                        index,
                        label: post.producer().to_string(),
                        output_anchors: Vec::new(),
                        readiness_targets: Vec::new(),
                    },
                })
                .collect(),
        }
    }

    /// Whether any stage in this plan must be executed by the worker.
    #[must_use]
    pub fn has_custom_stages(&self) -> bool {
        fn any<B>(stages: &[PlanStage<B>]) -> bool {
            stages
                .iter()
                .any(|stage| matches!(stage, PlanStage::Custom { .. }))
        }
        any(&self.sources) || any(&self.transforms) || any(&self.targets) || any(&self.posts)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{BuiltinTransform, PlanStage, StagePlan};
    use crate::sdk::builtins::SetTitle;

    #[test]
    fn builtin_declarations_round_trip_through_json() {
        let spec = BuiltinTransform::SetTitle(SetTitle::new("Bookstore API"));
        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains("\"stage\":\"set_title\""), "{json}");
        let back: BuiltinTransform = serde_json::from_str(&json).unwrap();
        assert_eq!(back.label(), "SetTitle");
    }

    #[test]
    fn an_empty_plan_has_no_custom_stages() {
        assert!(!StagePlan::default().has_custom_stages());
    }

    #[test]
    fn a_custom_entry_round_trips_with_its_declarations() {
        let stage: PlanStage<BuiltinTransform> = PlanStage::Custom {
            index: 2,
            label: "my::Stage".to_string(),
            output_anchors: vec!["generated/API.md".to_string()],
            readiness_targets: Vec::new(),
        };
        let json = serde_json::to_string(&stage).unwrap();
        let back: PlanStage<BuiltinTransform> = serde_json::from_str(&json).unwrap();
        let PlanStage::Custom {
            index,
            label,
            output_anchors,
            ..
        } = back
        else {
            panic!("expected a custom stage");
        };
        assert_eq!(index, 2);
        assert_eq!(label, "my::Stage");
        assert_eq!(output_anchors, vec!["generated/API.md".to_string()]);
    }
}
