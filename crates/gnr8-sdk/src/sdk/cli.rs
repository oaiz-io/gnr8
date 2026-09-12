//! Configuration for a generated command-line client emitted beside a generated SDK.
//!
//! This is the CLI gnr8 GENERATES for the user's API. It is unrelated to gnr8's own
//! `gnr8 init` / `generate` / `watch` command surface.

use crate::sdk::builtins::OperationSelector;

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
}

impl SdkCli {
    /// A generated-CLI option invoked as `program`.
    #[must_use]
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            commands: None,
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

    use super::SdkCli;
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
        let cli = SdkCli::new("bookstore");
        let json = serde_json::to_string(&cli).expect("serialize");
        let back: SdkCli = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(cli, back);
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
