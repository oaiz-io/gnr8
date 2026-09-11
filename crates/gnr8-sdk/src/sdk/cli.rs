//! Configuration for a generated command-line client emitted beside a generated SDK.
//!
//! This is the CLI gnr8 GENERATES for the user's API. It is unrelated to gnr8's own
//! `gnr8 init` / `generate` / `watch` command surface.

/// Configuration for a generated command-line client emitted beside a generated SDK.
///
/// This is the CLI gnr8 GENERATES for the user's API. It is unrelated to gnr8's own
/// `gnr8 init` / `generate` / `watch` command surface.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SdkCli {
    /// Program name — `argparse(prog=…)` and the `[project.scripts]` key.
    pub program: String,
}

impl SdkCli {
    /// A generated-CLI option invoked as `program`.
    #[must_use]
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SdkCli;
    use crate::sdk::builtins::PySdk;

    #[test]
    fn cli_sets_the_field() {
        let sdk = PySdk::new().cli("bookstore");
        assert_eq!(
            sdk.cli,
            Some(SdkCli {
                program: "bookstore".to_string()
            })
        );
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
}
