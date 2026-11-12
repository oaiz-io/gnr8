//! Each SDK target emits only the models its own operations reach.
//!
//! The `model-reach` fixture puts one model in each position the walk must follow:
//!
//! | model                         | reached                                                     |
//! |-------------------------------|-------------------------------------------------------------|
//! | `CatalogProduct`              | only through `OrderLine`, a nested array item               |
//! | `OrderStatus`, `SortOrder`    | an enum, only through a field and a query parameter         |
//! | `PaymentMethod`               | a union, whose variants `CardPayment` and `BankTransfer` are reached only through it |
//! | `GiftTag`                     | only as a map value                                         |
//! | `TreeNode`, `TreeEdge`        | recursive and mutually recursive                            |
//! | `OrderTotal` → `MoneyAmount`  | an alias, and the schema it names                           |
//! | `CouponDiscount`              | only through a nullable field                               |
//! | `OrderProblem`                | only as a 404 error body                                    |
//! | `OrderDraft`                  | only as a request body                                      |
//! | `AuditLog`, `AuditEntry`      | only by `listAudit`, which the targets here exclude         |
//! | `UnusedThing`, `UnusedChild`  | by no operation                                             |
//! | `OrderWebhook`                | by no operation; the targets declare it as a model root, and it reaches `WebhookPayload` and `Order` |
//!
//! Go has no sum types, so the Go SDK reads the fixture without `PaymentMethod` and its variants.
//!
//! Each case also builds what it emits: `go build`, a Python import of every reached model from the
//! package root, and `tsc` over a file importing every reached model from the index. A dangling
//! reference or a missing export fails there. A missing toolchain skips that step.

// Tests legitimately use unwrap/expect/panic (rust-best-practices skill ch.4).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use gnr8_engine::graph::ApiGraph;
use gnr8_engine::sdk::builtins::{generate_target, PlanTargets};
use gnr8_engine::sdk::prelude::*;
use gnr8_engine::sdk::{Artifacts, BuiltinTarget, Cx};
use gnr8_engine::CoreError;

const FIXTURE: &str = include_str!("../../../fixtures/sdk-targets/model-reach.json");

/// What every target in this file keeps, beside its operations' own bodies.
const REACHED: [&str; 18] = [
    "BankTransfer",
    "CardPayment",
    "CatalogProduct",
    "CouponDiscount",
    "GiftTag",
    "MoneyAmount",
    "Order",
    "OrderDraft",
    "OrderLine",
    "OrderProblem",
    "OrderStatus",
    "OrderTotal",
    "OrderWebhook",
    "PaymentMethod",
    "SortOrder",
    "TreeEdge",
    "TreeNode",
    "WebhookPayload",
];

/// Reached only by the excluded operation.
const EXCLUDED: [&str; 2] = ["AuditEntry", "AuditLog"];

/// Reached by nothing.
const UNREACHED: [&str; 2] = ["UnusedChild", "UnusedThing"];

/// The union and its variants, which the Go fixture leaves out.
const UNION: [&str; 3] = ["BankTransfer", "CardPayment", "PaymentMethod"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sdk {
    Go,
    Python,
    TypeScript,
}

const SDKS: [Sdk; 3] = [Sdk::Go, Sdk::Python, Sdk::TypeScript];

fn graph(sdk: Sdk) -> ApiGraph {
    let mut graph: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    if sdk == Sdk::Go {
        let schemas = graph["schemas"].as_array_mut().unwrap();
        schemas.retain(|schema| !UNION.contains(&schema["name"].as_str().unwrap()));
        for schema in schemas {
            if schema["id"] == "shop.Order" {
                let fields = schema["body"]["of"].as_array_mut().unwrap();
                fields.retain(|field| field["json_name"] != "payment");
            }
        }
    }
    serde_json::from_value(graph).expect("model-reach fixture must deserialize")
}

/// The models `sdk` can emit from `names`.
fn expressible(sdk: Sdk, names: &[&'static str]) -> Vec<&'static str> {
    names
        .iter()
        .copied()
        .filter(|name| sdk != Sdk::Go || !UNION.contains(name))
        .collect()
}

fn admin() -> OperationSelector {
    OperationSelector::PathPrefix("/admin/".to_string())
}

/// A target for `sdk` writing under `dir`, configured by the same calls in every language.
macro_rules! sdk_target {
    ($sdk:expr, $dir:expr, |$t:ident| $configure:expr) => {
        match $sdk {
            Sdk::Go => BuiltinTarget::GoSdk({
                let $t = GoSdk::new()
                    .module("example.com/shop/sdk")
                    .to($dir)
                    .without_contract_tests();
                $configure
            }),
            Sdk::Python => BuiltinTarget::PySdk({
                let $t = PySdk::new()
                    .module("shop")
                    .to($dir)
                    .without_contract_tests();
                $configure
            }),
            Sdk::TypeScript => BuiltinTarget::TsSdk({
                let $t = TsSdk::new()
                    .module("shop")
                    .to($dir)
                    .without_contract_tests();
                $configure
            }),
        }
    };
}

/// Every file one target writes, as `(path, text)`, in a plan of `plan`.
fn try_emit(
    target: &BuiltinTarget,
    graph: &ApiGraph,
    plan: &[&BuiltinTarget],
) -> Result<Vec<(String, String)>, CoreError> {
    let targets: Vec<(usize, &BuiltinTarget)> = plan.iter().copied().enumerate().collect();
    let mut out = Artifacts::new();
    generate_target(
        target,
        graph,
        &mut out,
        &Cx::new(std::env::temp_dir()),
        None,
        &PlanTargets::new(&targets),
    )?;
    Ok(out
        .into_files()
        .into_iter()
        .map(|artifact| (artifact.path, artifact.text))
        .collect())
}

fn emit(target: &BuiltinTarget, graph: &ApiGraph) -> Vec<(String, String)> {
    try_emit(target, graph, &[target]).expect("target must generate")
}

/// Whether one of `files` declares the model `name`.
fn declares(sdk: Sdk, files: &[(String, String)], name: &str) -> bool {
    let spellings = match sdk {
        Sdk::Go => vec![format!("\ntype {name} ")],
        Sdk::Python => vec![format!("\nclass {name}("), format!("\n{name} = ")],
        Sdk::TypeScript => vec![
            format!("export interface {name} "),
            format!("export type {name} ="),
            format!("export const {name} ="),
        ],
    };
    files
        .iter()
        .any(|(_, text)| spellings.iter().any(|spelling| text.contains(spelling)))
}

/// Whether any of `files` mentions `name` at all: a model, an export, an import, or a docs line.
fn mentions(files: &[(String, String)], name: &str) -> bool {
    files.iter().any(|(_, text)| text.contains(name))
}

fn assert_models(sdk: Sdk, files: &[(String, String)], present: &[&str], absent: &[&str]) {
    let paths: Vec<&str> = files.iter().map(|(path, _)| path.as_str()).collect();
    for name in present {
        assert!(
            declares(sdk, files, name),
            "{sdk:?} must declare {name}; files: {paths:?}"
        );
    }
    for name in absent {
        assert!(
            !mentions(files, name),
            "{sdk:?} must not mention {name}; files: {paths:?}"
        );
    }
}

#[test]
fn each_sdk_emits_only_the_models_its_operations_and_declared_roots_reach() {
    for sdk in SDKS {
        for split in [false, true] {
            let target = sdk_target!(sdk, "sdk", |t| {
                let t = t.exclude(admin()).model_root("shop.OrderWebhook");
                if split {
                    t.split_files()
                } else {
                    t
                }
            });
            let graph = graph(sdk);
            let files = emit(&target, &graph);
            let absent: Vec<&str> = EXCLUDED.iter().chain(&UNREACHED).copied().collect();
            assert_models(sdk, &files, &expressible(sdk, &REACHED), &absent);
            build(sdk, &files, &expressible(sdk, &REACHED));
        }
    }
}

#[test]
fn without_a_declared_root_the_webhook_models_are_not_emitted() {
    for sdk in SDKS {
        let target = sdk_target!(sdk, "sdk", |t| t.exclude(admin()));
        let files = emit(&target, &graph(sdk));
        assert_models(
            sdk,
            &files,
            &expressible(sdk, &["Order", "PaymentMethod", "TreeNode"]),
            &["OrderWebhook", "WebhookPayload", "AuditLog", "UnusedThing"],
        );
    }
}

#[test]
fn a_root_may_name_a_model_by_its_bare_name() {
    for sdk in SDKS {
        let target = sdk_target!(sdk, "sdk", |t| t.model_root("OrderWebhook"));
        let files = emit(&target, &graph(sdk));
        assert_models(
            sdk,
            &files,
            &["OrderWebhook", "WebhookPayload", "AuditLog"],
            &["UnusedThing"],
        );
    }
}

#[test]
fn all_models_emits_every_model_but_those_only_excluded_operations_reach() {
    for sdk in SDKS {
        let every: Vec<&str> = REACHED
            .iter()
            .chain(&EXCLUDED)
            .chain(&UNREACHED)
            .copied()
            .collect();

        let target = sdk_target!(sdk, "sdk", |t| t.all_models());
        let files = emit(&target, &graph(sdk));
        assert_models(sdk, &files, &expressible(sdk, &every), &[]);
        build(sdk, &files, &expressible(sdk, &every));

        let target = sdk_target!(sdk, "sdk", |t| t.exclude(admin()).all_models());
        let files = emit(&target, &graph(sdk));
        let kept: Vec<&str> = REACHED.iter().chain(&UNREACHED).copied().collect();
        assert_models(sdk, &files, &expressible(sdk, &kept), &EXCLUDED);
    }
}

#[test]
fn an_allowlist_target_and_a_full_target_keep_their_own_models() {
    for sdk in SDKS {
        let graph = graph(sdk);
        // Everything but `getOrder` is left out.
        let only_get_order =
            OperationSelector::Not(Box::new(OperationSelector::operation("getOrder")));
        let allowlist = sdk_target!(sdk, "allowlist", |t| t.exclude(only_get_order.clone()));
        let full = sdk_target!(sdk, "full", |t| t);
        let plan = [&allowlist, &full];

        let allowlist_files = try_emit(&allowlist, &graph, &plan).unwrap();
        assert_models(
            sdk,
            &allowlist_files,
            &expressible(
                sdk,
                &["Order", "OrderProblem", "SortOrder", "CatalogProduct"],
            ),
            &["OrderDraft", "AuditLog", "OrderWebhook", "UnusedThing"],
        );
        let full_files = try_emit(&full, &graph, &plan).unwrap();
        assert_models(
            sdk,
            &full_files,
            &["Order", "OrderDraft", "AuditLog", "AuditEntry", "SortOrder"],
            &["OrderWebhook", "UnusedThing"],
        );

        assert_eq!(
            allowlist_files,
            emit(&allowlist, &graph),
            "{sdk:?}: the full target beside it must not change the allowlist target"
        );
        assert_eq!(
            full_files,
            emit(&full, &graph),
            "{sdk:?}: the allowlist target beside it must not change the full target"
        );
    }
}

#[test]
fn a_root_that_names_no_schema_is_a_configuration_error() {
    for sdk in SDKS {
        let target = sdk_target!(sdk, "sdk", |t| t.model_root("shop.Gone"));
        let error = try_emit(&target, &graph(sdk), &[&target])
            .expect_err("a stale model root must fail")
            .to_string();
        assert!(error.contains("shop.Gone"), "{error}");
        assert!(error.contains("model root"), "{error}");
    }
}

#[test]
fn a_root_beside_all_models_is_a_configuration_error() {
    for sdk in SDKS {
        let target = sdk_target!(sdk, "sdk", |t| t
            .all_models()
            .model_root("shop.OrderWebhook"));
        let error = try_emit(&target, &graph(sdk), &[&target])
            .expect_err("a root beside all_models must fail")
            .to_string();
        assert!(error.contains("all_models"), "{error}");
        assert!(error.contains("shop.OrderWebhook"), "{error}");
    }
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_temp_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let seq = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "gnr8-model-reach-{label}-{}-{seq}-{nanos}",
        std::process::id(),
    ));
    std::fs::create_dir_all(&dir).expect("create unique temp dir");
    dir
}

/// Write `files` (each under the target's `sdk/` dir) into `root`.
fn write(files: &[(String, String)], root: &Path) {
    for (path, text) in files {
        let relative = path.split_once('/').map_or(path.as_str(), |(_, rest)| rest);
        let destination = root.join(relative);
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        std::fs::write(destination, text).unwrap();
    }
}

fn run(program: &str, args: &[&str], dir: &Path, envs: &[(&str, &str)]) -> Option<String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(dir)
        .envs(envs.iter().copied())
        .output()
        .ok()?;
    assert!(
        output.status.success(),
        "{program} {args:?} failed in {}:\n{}\n{}",
        dir.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Build the emitted SDK and import `models` from its public surface.
fn build(sdk: Sdk, files: &[(String, String)], models: &[&str]) {
    let dir = unique_temp_dir(&format!("{sdk:?}").to_lowercase());
    match sdk {
        Sdk::Go => {
            write(files, &dir);
            let envs = [("GOPROXY", "off"), ("GOFLAGS", "-mod=mod")];
            if run("go", &["build", "./..."], &dir, &envs).is_none() {
                eprintln!("skipping the Go build: go is not available");
            }
        }
        Sdk::Python => {
            write(files, &dir.join("shop"));
            std::fs::write(dir.join("pydantic.py"), PYDANTIC_STUB).unwrap();
            let import = format!("from shop import {}", models.join(", "));
            let envs = [("PYTHONDONTWRITEBYTECODE", "1"), ("PYTHONNOUSERSITE", "1")];
            if run("python3", &["-c", &import], &dir, &envs).is_none() {
                eprintln!("skipping the Python import: python3 is not available");
            }
        }
        Sdk::TypeScript => {
            let tsc = concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tsextract/node_modules/typescript/bin/tsc"
            );
            if Path::new(tsc).exists() {
                write(files, &dir);
                let uses: Vec<String> = models
                    .iter()
                    .map(|name| format!("export type Use{name} = {name};"))
                    .collect();
                let consumer = format!(
                    "import type {{ {} }} from \"./index\";\n{}\n",
                    models.join(", "),
                    uses.join("\n")
                );
                std::fs::write(dir.join("consumer.ts"), consumer).unwrap();
                let args = [
                    tsc,
                    "--noEmit",
                    "--strict",
                    "--target",
                    "es2022",
                    "--module",
                    "esnext",
                    "--moduleResolution",
                    "bundler",
                    "--lib",
                    "es2022,dom",
                    "consumer.ts",
                ];
                if run("node", &args, &dir, &[]).is_none() {
                    eprintln!("skipping tsc: node is not available");
                }
            } else {
                eprintln!("skipping tsc: {tsc} is not installed");
            }
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Enough of Pydantic for a generated package to import: the class bodies run, nothing validates.
const PYDANTIC_STUB: &str = r"class ConfigDict(dict):
    pass


def Field(default=None, *args, **kwargs):
    return default


class ValidationError(ValueError):
    pass


class BaseModel:
    def __init__(self, **kwargs):
        for name, value in kwargs.items():
            setattr(self, name, value)
";
