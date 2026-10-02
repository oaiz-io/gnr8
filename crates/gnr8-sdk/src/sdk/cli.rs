//! Configuration for a generated command-line client emitted beside a generated SDK.
//!
//! This is the CLI gnr8 GENERATES for the user's API. It is unrelated to gnr8's own
//! `gnr8 init` / `generate` / `watch` command surface.

use crate::sdk::builtins::OperationSelector;

fn default_true() -> bool {
    true
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_true(value: &bool) -> bool {
    *value
}

/// Configuration for a generated command-line client emitted beside a generated SDK.
///
/// This is the CLI gnr8 GENERATES for the user's API. It is unrelated to gnr8's own
/// `gnr8 init` / `generate` / `watch` command surface.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SdkCli {
    /// Program name — Python `argparse(prog=…)` / `[project.scripts]` key, and the Go
    /// `cmd/<program>/` directory. Go has no `[project.scripts]` equivalent: `.cli()` on
    /// [`crate::sdk::builtins::GoSdk`] does not require package metadata, because
    /// `cmd/<program>/main.go` compiles standalone.
    pub program: String,
    /// Which operations become commands. `None` means every operation in the graph.
    ///
    /// This selects which facts the program wraps; it never renames one. An operation left out is
    /// still in the OpenAPI document and still a method on the generated client — the CLI simply
    /// does not wrap it, the way a hand-written CLI wraps part of the SDK it calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commands: Option<OperationSelector>,
    /// The host this program talks to unless `--base-url` says otherwise.
    ///
    /// `None` means the program has no default and `--base-url` is required on every command. The
    /// OpenAPI document's `servers` is deliberately not consulted: what a program points at is a
    /// fact about the program, and deriving it from what the document advertises made the only way
    /// to set it a change to the published contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Whether to emit `cmd/<program>/main.go`.
    ///
    /// `true` (the default) writes a minimal `package main` that stamps `version`/`commit`/`date`
    /// as variables — so `-ldflags -X` can overwrite them — and calls `Run`. `false` skips that
    /// file: the program's `main` is hand-owned, lives beside the generated tree, and is never
    /// deleted because gnr8 never emitted it.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub emit_main: bool,
    /// Commands the generated dispatcher names whose implementation is hand-owned.
    ///
    /// Each entry becomes a root dispatch arm that calls a function in `package cli`. gnr8 never
    /// writes that function, so a file the user adds for it is not in the ownership manifest and
    /// is never deleted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owned_commands: Vec<OwnedCommand>,
}

/// One hand-owned command the generated dispatcher names.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OwnedCommand {
    /// Invocation name (`login`, `status`). Kebab-case, like every other command.
    pub name: String,
    /// One-line summary for root help. Absent means the name stands alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Go function in `package cli` the dispatcher calls. Absent means `run` plus the exported
    /// form of [`Self::name`] (`login` → `runLogin`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
}

impl OwnedCommand {
    /// A hand-owned root command invoked as `name`.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            summary: None,
            function: None,
        }
    }

    /// The one line root help prints beside this command.
    #[must_use]
    pub fn summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    /// The Go function in `package cli` the dispatcher calls (`runLogin`).
    #[must_use]
    pub fn function(mut self, function: impl Into<String>) -> Self {
        self.function = Some(function.into());
        self
    }
}

impl From<&str> for OwnedCommand {
    fn from(name: &str) -> Self {
        Self::new(name)
    }
}

impl From<String> for OwnedCommand {
    fn from(name: String) -> Self {
        Self::new(name)
    }
}

impl SdkCli {
    /// A generated-CLI option invoked as `program`.
    #[must_use]
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            commands: None,
            base_url: None,
            emit_main: true,
            owned_commands: Vec::new(),
        }
    }

    /// Emit a command only for the operations `selector` matches.
    ///
    /// A selector that matches no operation is a configuration error, like every other selector
    /// consumer: a program with no commands is not a program.
    #[must_use]
    pub fn commands(mut self, selector: OperationSelector) -> Self {
        self.commands = Some(selector);
        self
    }

    /// The host every command talks to unless `--base-url` overrides it.
    ///
    /// Without this the program has no default and `--base-url` is required, which is the honest
    /// answer: a CLI that silently points a production client at `localhost` is worse than one
    /// that asks.
    #[must_use]
    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }

    /// Do not emit `cmd/<program>/main.go`. The program's `main` is hand-owned and calls `Run`.
    #[must_use]
    pub fn hand_owned_main(mut self) -> Self {
        self.emit_main = false;
        self
    }

    /// Name a root command whose implementation is hand-owned and never generated.
    #[must_use]
    pub fn owned_command(mut self, command: impl Into<OwnedCommand>) -> Self {
        self.owned_commands.push(command.into());
        self
    }
}

impl From<&str> for SdkCli {
    fn from(program: &str) -> Self {
        Self::new(program)
    }
}

impl From<String> for SdkCli {
    fn from(program: String) -> Self {
        Self::new(program)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{OwnedCommand, SdkCli};
    use crate::sdk::builtins::{GoSdk, PySdk};

    #[test]
    fn cli_sets_the_field() {
        let sdk = PySdk::new().cli("bookstore");
        assert_eq!(sdk.cli, Some(SdkCli::new("bookstore")));
        let go = GoSdk::new().cli("bookstore");
        assert_eq!(go.cli, Some(SdkCli::new("bookstore")));
    }

    #[test]
    fn declaration_serde_round_trips() {
        let cli = SdkCli::new("bookstore")
            .owned_command(OwnedCommand::new("login").summary("Sign in"))
            .hand_owned_main();
        let json = serde_json::to_string(&cli).expect("serialize");
        let back: SdkCli = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(cli, back);
        assert!(!back.emit_main);
        assert_eq!(back.owned_commands[0].name, "login");
    }

    #[test]
    fn absent_new_fields_deserialize_to_defaults() {
        let back: SdkCli = serde_json::from_str(r#"{"program":"bookstore"}"#).expect("deserialize");
        assert!(back.emit_main);
        assert!(back.owned_commands.is_empty());
    }

    #[test]
    fn pysdk_serialized_without_cli_deserializes_to_none() {
        let sdk = PySdk::new().module("example.com/bookstore/sdk").to("sdk");
        let json = serde_json::to_string(&sdk).expect("serialize");
        assert!(
            !json.contains("\"cli\""),
            "absent cli must skip serializing: {json}"
        );
        let back: PySdk = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.cli, None);
    }

    #[test]
    fn gosdk_serialized_without_cli_deserializes_to_none() {
        let sdk = GoSdk::new().module("example.com/bookstore/sdk").to("sdk");
        let json = serde_json::to_string(&sdk).expect("serialize");
        assert!(
            !json.contains("\"cli\""),
            "absent cli must skip serializing: {json}"
        );
        let back: GoSdk = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.cli, None);
    }
}
