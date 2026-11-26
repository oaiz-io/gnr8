//! Language-native identifier spelling and call shape across the three built-in SDK targets.
//!
//! One `OpenAPI` document drives all three emitters so the two facts this locks stay coupled to the
//! SAME graph:
//!
//! - **Go initialisms.** An initialism keeps FULL CAPS when pluralized (`stepUuids` → `StepUUIDs`,
//!   `labelIds` → `LabelIDs`). This is a Go-local spelling of the exported identifier; the wire token
//!   in the json tag, the query key, and the `OpenAPI` property name are untouched, and TypeScript and
//!   Python keep their own language-native casing.
//! - **TypeScript call shape.** Path params stay positional; every other request parameter arrives as
//!   ONE typed `{Operation}Params` object, with `RequestOptions` last.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use gnr8_engine::sdk::prelude::*;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/openapi-import/params-and-initialisms.yaml"
);
const TSC: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tsextract/node_modules/typescript/bin/tsc"
);

/// Distinguishes concurrent staging dirs: several tests share the `generate` label, and the clock
/// alone is not fine-grained enough to keep two threads that start together apart. Two threads that
/// picked the same directory raced, one deleting the fixture the other was still generating from.
static TEMP_DIR_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let sequence = TEMP_DIR_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "gnr8-call-shape-{label}-{}-{nanos}-{sequence}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Generate Go, Python, and TypeScript SDKs from the fixture, keyed by artifact path.
fn generate() -> BTreeMap<String, String> {
    run_pipeline("generate", Pipeline::new())
}

/// The same fixture with all three pagination modes configured, so the generated page/item helpers
/// are exercised too.
fn generate_paginated() -> BTreeMap<String, String> {
    run_pipeline(
        "paginated",
        Pipeline::new()
            .transform(ConfigurePagination::cursor(
                OperationSelector::operation("getItemsPaginated"),
                "cursor",
                "nextCursor",
                "items",
            ))
            .transform(ConfigurePagination::page(
                OperationSelector::operation("listItemsByPage"),
                "page",
                "perPage",
                "items",
            ))
            .transform(ConfigurePagination::offset(
                OperationSelector::operation("listItemsByOffset"),
                "offset",
                "limit",
                "items",
            )),
    )
}

fn run_pipeline(label: &str, pipeline: Pipeline) -> BTreeMap<String, String> {
    let root = temp_dir(label);
    std::fs::copy(Path::new(FIXTURE), root.join("openapi.yaml")).expect("copy fixture");
    let pipeline = pipeline
        .source(OpenApi::new().input("openapi.yaml"))
        .target(GoSdk::new().module("github.com/acme/fixture").to("go"))
        .target(PySdk::new().module("acme-fixture").to("python"))
        .target(TsSdk::new().module("@acme/fixture").to("ts"))
        .target(RustSdk::new().module("acme-fixture").to("rust"));
    let outcome = gnr8_engine::pipeline::run_in_process(&pipeline, &Cx::new(&root), None)
        .expect("pipeline must generate");
    let files = outcome
        .artifacts
        .iter()
        .map(|artifact| (artifact.path.clone(), artifact.text.clone()))
        .collect();
    let _ = std::fs::remove_dir_all(&root);
    files
}

fn file<'a>(files: &'a BTreeMap<String, String>, path: &str) -> &'a str {
    files
        .get(path)
        .unwrap_or_else(|| {
            panic!(
                "missing generated artifact {path}; got {:?}",
                files.keys().collect::<Vec<_>>()
            )
        })
        .as_str()
}

fn assert_contains(haystack: &str, needle: &str, what: &str) {
    assert!(
        haystack.contains(needle),
        "{what}: expected to find\n  {needle}\nin:\n{haystack}"
    );
}

fn assert_absent(haystack: &str, needle: &str, what: &str) {
    assert!(
        !haystack.contains(needle),
        "{what}: expected NOT to find\n  {needle}\nin:\n{haystack}"
    );
}

/// Strip every double-quoted and backquoted literal from generated Go, leaving the identifiers.
///
/// Wire tokens are quoted (`"excludedStepUuids"`, `` `json:"stepUuids"` ``) and are DELIBERATELY not
/// respelled, so a raw substring search for a wrong identifier would match the very wire name the
/// rename is required to preserve. Removing the literals lets the negative assertions say exactly what
/// they mean: no such IDENTIFIER exists.
///
/// A lone `'` is NOT treated as an opener: it appears in prose comments far more often than as a Go
/// rune literal, and mistaking one for a delimiter would swallow the rest of the file and make every
/// downstream "must be absent" assertion pass vacuously. Callers guard the remaining direction with
/// [`assert_survives_literal_stripping`].
fn identifiers_only(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in source.chars() {
        match quote {
            Some(open) => {
                if escaped {
                    escaped = false;
                } else if ch == '\\' && open != '`' {
                    escaped = true;
                } else if ch == open {
                    quote = None;
                }
            }
            None => {
                if ch == '"' || ch == '`' {
                    quote = Some(ch);
                } else {
                    out.push(ch);
                }
            }
        }
    }
    out
}

/// Prove the stripped source still carries the identifiers, so "must be absent" means something.
fn assert_survives_literal_stripping(stripped: &str, expected: &[&str], what: &str) {
    for identifier in expected {
        assert!(
            stripped.contains(identifier),
            "{what}: literal stripping swallowed real source — '{identifier}' is missing from:\n{stripped}"
        );
    }
}

#[test]
fn go_identifiers_keep_initialisms_capitalized_when_pluralized() {
    let files = generate();
    let models = file(&files, "go/models.go");
    let operations = file(&files, "go/operations.go");

    // Required Go field mapping. Left of the arrow is the wire token, right is the exported field.
    for (wire, field) in [
        ("uuid", "UUID"),
        ("stepUuids", "StepUUIDs"),
        ("createdByUuid", "CreatedByUUID"),
        ("primaryFileId", "PrimaryFileID"),
        ("labelIds", "LabelIDs"),
        ("siteUrls", "SiteURLs"),
        ("publicApis", "PublicAPIs"),
        ("ownerUuids", "OwnerUUIDs"),
    ] {
        assert_contains(
            models,
            &format!("{field} "),
            &format!("{wire} must emit the Go field {field}"),
        );
        assert_contains(
            models,
            &format!("`json:\"{wire}"),
            &format!("{wire} must keep its wire token"),
        );
    }

    // The half-capitalized spellings must not survive as Go IDENTIFIERS. Wire tokens are quoted and
    // stay exactly as the document spells them, so the literals are stripped before this check.
    let model_idents = identifiers_only(models);
    let operation_idents = identifiers_only(operations);
    assert_survives_literal_stripping(&model_idents, &["StepUUIDs", "LabelIDs"], "Go models");
    assert_survives_literal_stripping(
        &operation_idents,
        &["ExcludedStepUUIDs", "SearchPipelines"],
        "Go operations",
    );
    for wrong in [
        "StepUuids",
        "ExcludedStepUuids",
        "CreatedByUuid",
        "PrimaryFileId",
        "LabelIds",
        "OwnerIds",
        "OwnerUuids",
        "SiteUrls",
        "PublicApis",
        // Token-boundary damage from pluralizing then splitting.
        "UuiDs",
        "uui_ds",
    ] {
        assert_absent(&model_idents, wrong, "Go model identifiers");
        assert_absent(&operation_idents, wrong, "Go operation identifiers");
    }

    // Query params live on the per-operation params struct, with the WIRE name kept at the call site.
    assert_contains(
        operations,
        "ExcludedStepUUIDs",
        "search params struct field",
    );
    assert_contains(operations, "OwnerIDs", "search params struct field");
    assert_contains(operations, "\"excludedStepUuids\"", "query wire name");
    assert_contains(operations, "\"ownerIds\"", "query wire name");
}

#[test]
fn typescript_and_python_keep_language_native_identifiers() {
    let files = generate();
    let ts_models = file(&files, "ts/models.ts");
    let py_models = file(&files, "python/models.py");

    // The Go initialism spelling is Go-local: it must not leak into the other two targets.
    assert_contains(ts_models, "stepUuids: string[]", "required TS model field");
    for native in ["labelIds?:", "createdByUuid?:", "ownerUuids?:"] {
        assert_contains(ts_models, native, "optional TS model field");
    }
    for wrong in ["stepUUIDs", "StepUUIDs", "labelIDs", "UUIDs"] {
        assert_absent(ts_models, wrong, "TypeScript models");
    }

    for native in [
        "step_uuids",
        "label_ids",
        "created_by_uuid",
        "owner_uuids",
        "primary_file_id",
    ] {
        assert_contains(py_models, native, "Python model field");
    }
    for wrong in ["stepUUIDs", "step_UUIDs", "UUIDs"] {
        assert_absent(py_models, wrong, "Python models");
    }
}

#[test]
fn openapi_document_is_unchanged_by_go_identifier_spelling() {
    let root = temp_dir("openapi");
    std::fs::copy(Path::new(FIXTURE), root.join("openapi.yaml")).expect("copy fixture");
    let pipeline = Pipeline::new()
        .source(OpenApi::new().input("openapi.yaml"))
        .target(OpenApi31::new().to("out.yaml"));
    let outcome = gnr8_engine::pipeline::run_in_process(&pipeline, &Cx::new(&root), None)
        .expect("pipeline must generate");
    let document = outcome
        .artifacts
        .iter()
        .find(|artifact| artifact.path == "out.yaml")
        .expect("openapi artifact")
        .text
        .clone();
    let _ = std::fs::remove_dir_all(&root);

    for wire in [
        "stepUuids",
        "labelIds",
        "createdByUuid",
        "excludedStepUuids",
        "ownerIds",
    ] {
        assert_contains(&document, wire, "OpenAPI property/parameter name");
    }
    for wrong in ["stepUUIDs", "StepUUIDs", "labelIDs", "OwnerIDs"] {
        assert_absent(&document, wrong, "OpenAPI document");
    }
}

#[test]
fn typescript_takes_one_params_object_for_every_non_path_parameter() {
    let files = generate();
    let client = file(&files, "ts/client.ts");
    let index = file(&files, "ts/index.ts");

    // Query-only, all optional → `op(params?: OpParams, options?: RequestOptions)`.
    assert_contains(
        client,
        "  async getItemsPaginated(\n    params?: GetItemsPaginatedParams,\n    options?: RequestOptions,\n  ): Promise<models.ItemPage> {",
        "query-only all-optional signature",
    );
    assert_absent(client, "getItemsPaginated(\n    kinds", "positional query");
    assert_contains(
        client,
        "export type GetItemsPaginatedParams = {\n  cursor?: string;\n  kinds?: string[];\n  pageSize?: number;\n  query?: string;\n  statuses?: string[];\n};",
        "params type",
    );
    // Wire names are untouched by the params-object shape.
    for wire in ["\"cursor\"", "\"pageSize\"", "\"statuses\""] {
        assert_contains(client, wire, "query wire name");
    }

    // Query-only, all optional, with the ticket's array/explode parameters.
    assert_contains(
        client,
        "  async searchPipelines(\n    params?: SearchPipelinesParams,\n    options?: RequestOptions,\n  ): Promise<models.PipelineList> {",
        "searchPipelines signature",
    );
    assert_contains(
        client,
        "export type SearchPipelinesParams = {\n  excludedStepUuids?: string[];\n  ownerIds?: string[];\n};",
        "searchPipelines params type",
    );

    // Query-only with a REQUIRED query param → the params object itself is required.
    assert_contains(
        client,
        "  async exportItems(\n    params: ExportItemsParams,\n    options?: RequestOptions,\n  ): Promise<models.ItemPage> {",
        "required-query signature",
    );
    assert_contains(
        client,
        "export type ExportItemsParams = {\n  format: string;\n  locale?: string;\n};",
        "required-query params type",
    );

    // Path + optional query → the path param stays positional and comes first.
    assert_contains(
        client,
        "  async getItem(\n    itemId: string,\n    params?: GetItemParams,\n    options?: RequestOptions,\n  ): Promise<models.Item> {",
        "path + query signature",
    );
    // A wire name whose camelCase form is not a legal bare member name is quoted as a property, is
    // probed through an optional-chained index, and is read by index — the wire name is unchanged.
    assert_contains(
        client,
        "export type GetItemParams = {\n  \"2fa\"?: boolean;\n  verbose?: boolean;\n};",
        "non-identifier params key",
    );
    assert_contains(
        client,
        "    if (params?.[\"2fa\"] !== undefined) {",
        "non-identifier params probe",
    );
    assert_contains(
        client,
        "      params[\"2fa\"],",
        "non-identifier params read",
    );

    // Body + optional query → the typed body stays its own positional argument.
    assert_contains(
        client,
        "  async createItem(\n    body: models.ItemInput,\n    params?: CreateItemParams,\n    options?: RequestOptions,\n  ): Promise<models.Item> {",
        "body + query signature",
    );

    // Path + body + optional query.
    assert_contains(
        client,
        "  async replaceItem(\n    itemId: string,\n    body: models.ItemInput,\n    params?: ReplaceItemParams,\n    options?: RequestOptions,\n  ): Promise<models.Item> {",
        "path + body + query signature",
    );

    // No request parameters at all → no useless params argument, and no params type.
    assert_contains(
        client,
        "  async getHealth(options?: RequestOptions): Promise<models.Health> {",
        "parameterless signature",
    );
    assert_absent(client, "GetHealthParams", "parameterless op");

    // Every params type is re-exported from the package root, so callers can name the argument.
    assert_contains(
        index,
        "export type {\n  CreateItemParams,\n  ExportItemsParams,\n  GetItemParams,\n  GetItemsPaginatedParams,\n  ListItemsByOffsetParams,\n  ListItemsByPageParams,\n  ReplaceItemParams,\n  SearchPipelinesParams,\n} from \"./client\";",
        "index re-exports",
    );
}

/// The generated pagination helpers advance a COPY of the caller's params object.
#[test]
fn typescript_pagination_helpers_advance_a_copy_of_the_params_object() {
    let files = generate_paginated();
    let client = file(&files, "ts/client.ts");

    // Cursor mode: seed nothing, write the next cursor back onto the copy.
    assert_contains(
        client,
        "    const pageParams: GetItemsPaginatedParams = { ...params };\n    while (true) {\n      const page = await this.getItemsPaginated(pageParams, options);",
        "cursor-mode page params",
    );
    assert_contains(
        client,
        "      pageParams.cursor = nextCursor;",
        "cursor advance",
    );
    // A cursor loop that stops on a missing next cursor never reads the page's item list, so it must
    // not bind one — a dead local fails any consumer compiling with `noUnusedLocals`.
    assert_contains(
        client,
        "      const page = await this.getItemsPaginated(pageParams, options);\n      yield page;",
        "no dead item local in the cursor generator",
    );
    // Offset mode DOES read it, to advance by the page length.
    assert_contains(
        client,
        "      const items = page.items ?? [];",
        "offset generator binds the item list it reads",
    );
    // The item generator forwards the CALLER's params, not the page-local copy.
    assert_contains(
        client,
        "for await (const page of this.getItemsPaginatedPages(params, options)) {",
        "item generator delegation",
    );

    // Page mode: seed page 1 when the caller left it unset, then step it.
    assert_contains(
        client,
        "    const pageParams: ListItemsByPageParams = { ...params };\n    if (pageParams.page === undefined) {\n      pageParams.page = 1;\n    }",
        "page-mode seed",
    );
    assert_contains(client, "      pageParams.page += 1;", "page advance");

    // Offset mode: seed offset 0, then advance by the page length.
    assert_contains(
        client,
        "    const pageParams: ListItemsByOffsetParams = { ...params };\n    if (pageParams.offset === undefined) {\n      pageParams.offset = 0;\n    }",
        "offset-mode seed",
    );
    assert_contains(
        client,
        "      pageParams.offset += items.length;",
        "offset advance",
    );
}

#[test]
fn generating_twice_is_byte_identical() {
    assert_eq!(generate(), generate(), "regeneration must be a no-op");
}

fn toolchain_available(bin: &str, arg: &str) -> bool {
    Command::new(bin)
        .arg(arg)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

fn run_checked(mut command: Command) -> Result<(), String> {
    let output = command.output().map_err(|error| error.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let mut message = String::from_utf8_lossy(&output.stdout).into_owned();
    message.push_str(&String::from_utf8_lossy(&output.stderr));
    Err(message)
}

fn write_files(files: &BTreeMap<String, String>, root: &Path, prefix: &str) {
    for (path, text) in files {
        let Some(rest) = path.strip_prefix(prefix) else {
            continue;
        };
        let target = root.join(rest.trim_start_matches('/'));
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(target, text).expect("write file");
    }
}

/// The renamed Go fields must still compile, and the params-object TypeScript must still typecheck.
///
/// The pagination-configured graph is the one checked here: it is a strict superset of the plain one
/// (same operations plus the page/item generators), so it covers both in a single toolchain run.
#[test]
fn generated_go_and_typescript_compile() {
    let files = generate_paginated();

    if toolchain_available("go", "version") {
        let go_root = temp_dir("go-build");
        eprintln!("sdk_call_shape: running `go build` over the generated Go SDK");
        write_files(&files, &go_root, "go/");
        run_checked({
            let mut cmd = Command::new("go");
            cmd.arg("build").arg("./...").current_dir(&go_root);
            cmd
        })
        .expect("generated Go SDK must build");
        let _ = std::fs::remove_dir_all(&go_root);
    } else {
        eprintln!("sdk_call_shape: skipping `go build` — go toolchain unavailable");
    }

    if toolchain_available("node", "--version") && Path::new(TSC).is_file() {
        eprintln!("sdk_call_shape: typechecking the generated TypeScript SDK with tsc --strict");
        let ts_root = temp_dir("ts-check");
        write_files(&files, &ts_root, "ts/");
        // A driver that exercises the documented call shapes, so the test fails if a caller can no
        // longer write them.
        std::fs::write(
            ts_root.join("driver.ts"),
            r#"import { Client } from "./client";
import type { GetItemsPaginatedParams } from "./index";

export async function drive(client: Client): Promise<void> {
  await client.getItemsPaginated({ pageSize: 50, cursor: "abc" });
  await client.getItemsPaginated();
  const params: GetItemsPaginatedParams = { statuses: ["ready"] };
  await client.getItemsPaginated(params, { timeoutMs: 1_000 });
  await client.searchPipelines({ ownerIds: ["a", "b"] });
  await client.exportItems({ format: "csv" });
  await client.getItem("item-1");
  await client.getItem("item-1", { verbose: true, "2fa": true });
  await client.createItem({ kind: "job" }, { notify: true });
  await client.replaceItem("item-1", { kind: "job" }, { dryRun: true });
  await client.getHealth();

  // Pagination generators keep the same call shape as the operation they wrap.
  for await (const page of client.getItemsPaginatedPages({ pageSize: 10 })) {
    void page.items.length;
  }
  for await (const item of client.iterateListItemsByPage({ perPage: 10 })) {
    void item.uuid;
  }
  for await (const item of client.iterateListItemsByOffset()) {
    void item.uuid;
  }
}
"#,
        )
        .expect("write driver");
        run_checked({
            let mut cmd = Command::new("node");
            cmd.arg(TSC)
                .args([
                    // The same strict set `tssdk_compile` gates the other fixtures with, so this
                    // fixture is held to one bar and not a looser one.
                    "--noEmit",
                    "--strict",
                    "--noUnusedLocals",
                    "--exactOptionalPropertyTypes",
                    "--noUncheckedIndexedAccess",
                    "--target",
                    "es2022",
                    "--module",
                    "esnext",
                    "--moduleResolution",
                    "bundler",
                    "--lib",
                    "es2022,dom",
                    "driver.ts",
                ])
                .current_dir(&ts_root);
            cmd
        })
        .expect("generated TypeScript SDK and driver must typecheck");
        let _ = std::fs::remove_dir_all(&ts_root);
    } else {
        eprintln!("sdk_call_shape: skipping tsc — node/typescript toolchain unavailable");
    }
}
#[test]
fn rust_keeps_language_native_identifiers_and_one_params_struct() {
    let files = generate_paginated();
    let models = file(&files, "rust/src/models.rs");
    let operations = file(&files, "rust/src/operations.rs");
    for native in [
        "pub step_uuids: Vec<String>,",
        "pub label_ids: Option<Vec<String>>,",
        "pub created_by_uuid: Option<String>,",
        "pub owner_uuids: Option<Vec<String>>,",
    ] {
        assert_contains(models, native, "Rust model field");
    }
    for wrong in ["stepUUIDs", "step_UUIDs", "UUIDs"] {
        assert_absent(models, wrong, "Rust models");
    }
    // Every query, header and cookie parameter is a field of one struct, and a name that starts
    // with a digit gets a letter in front.
    assert_contains(operations, "pub struct GetItemParams {", "params struct");
    assert_contains(
        operations,
        "pub value_2fa: Option<bool>,",
        "digit-led field",
    );
    for helper in [
        "pub fn get_items_paginated_pages(",
        "pub fn iter_get_items_paginated(",
        "pub fn list_items_by_page_pages(",
        "pub fn iter_list_items_by_offset(",
    ] {
        assert_contains(operations, helper, "pagination helper");
    }
}

/// The Rust SDK compiles offline with a driver that writes each call shape a caller writes.
#[test]
fn generated_rust_compiles_with_a_driver() {
    const PACKAGE: &str = "name = \"jobs-consumer\"\n";
    let files = generate_paginated();
    let root = temp_dir("rust-check");
    write_files(&files, &root, "rust/");
    let manifest = std::fs::read_to_string(root.join("Cargo.toml"))
        .expect("read manifest")
        .replace("name = \"acmefixture\"\n", "name = \"call-shape-rust\"\n");
    std::fs::write(
        root.join("Cargo.toml"),
        format!("{manifest}\n[workspace]\n"),
    )
    .expect("write manifest");
    let lock = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/rust-models/consumer/Cargo.lock"),
    )
    .expect("read lockfile")
    .replace(PACKAGE, "name = \"call-shape-rust\"\n");
    std::fs::write(root.join("Cargo.lock"), lock).expect("write lockfile");
    let mut lib = std::fs::read_to_string(root.join("src/lib.rs")).expect("read lib.rs");
    lib.push_str(
        r#"
#[allow(dead_code, clippy::all)]
fn drive(client: &Client) -> Result<(), Error> {
    let mut params = GetItemsPaginatedParams::new();
    params.page_size = Some(50);
    params.cursor = Some("abc".to_string());
    client.get_items_paginated(&params)?;
    client.export_items(&ExportItemsParams::new("csv".to_string()))?;
    let mut flags = GetItemParams::default();
    flags.value_2fa = Some(true);
    client.get_item("item-1", &flags)?;
    client.get_health()?;
    for page in client.get_items_paginated_pages(&GetItemsPaginatedParams::new()) {
        let _ = page?.items.len();
    }
    for item in client.iter_list_items_by_page(&ListItemsByPageParams::new()) {
        let _ = item?.uuid;
    }
    for item in client.iter_list_items_by_offset(&ListItemsByOffsetParams::new()) {
        let _ = item?.uuid;
    }
    Ok(())
}
"#,
    );
    std::fs::write(root.join("src/lib.rs"), lib).expect("write lib.rs");
    run_checked({
        let mut cmd = Command::new(env!("CARGO"));
        cmd.args(["check", "--quiet", "--offline", "--locked"])
            .current_dir(&root)
            .env(
                "CARGO_TARGET_DIR",
                Path::new(env!("CARGO_TARGET_TMPDIR")).join("rust-models-target"),
            );
        cmd
    })
    .expect("generated Rust SDK and driver must compile");
    let _ = std::fs::remove_dir_all(&root);
}
