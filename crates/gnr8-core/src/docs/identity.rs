//! What a consumer imports and installs: the one consumer identity per sibling SDK.
//!
//! Every place a page, a sample heading, a compile unit or a README names an SDK package reads it
//! here, and it is computed by the same functions the SDK target's own package manifest writer
//! uses, so a page can never name a package the SDK does not publish.

use crate::sdk::builtins::{sdk_package, SiblingSdk};
use crate::verify::ContractTestLanguage;
use crate::CoreError;

/// What a consumer imports and installs. Exists only when the SDK target emits a package manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumerIdentity {
    /// The import specifier: the Go module path, the Python import package, the npm package name.
    pub(crate) import: String,
    /// The name the consumer's code spells the package's symbols with (Go: the package clause).
    pub(crate) qualifier: String,
    /// The name a consumer installs: the Go module path, the Python distribution name
    /// `pyproject.toml` lists, the `package.json` name.
    pub(crate) install: String,
    /// The SDK's language, which decides the install tool.
    pub(crate) language: ContractTestLanguage,
}

impl ConsumerIdentity {
    /// The import specifier the samples print.
    #[must_use]
    pub fn import(&self) -> &str {
        &self.import
    }

    /// The name a consumer installs, exactly as the SDK's package manifest declares it.
    #[must_use]
    pub fn install(&self) -> &str {
        &self.install
    }

    /// The command that installs the package with the language's own tool.
    #[must_use]
    pub fn install_command(&self) -> String {
        let tool = match self.language {
            ContractTestLanguage::Go => "go get",
            ContractTestLanguage::Python => "pip install",
            ContractTestLanguage::TypeScript => "npm install",
        };
        format!("{tool} {}", self.install)
    }
}

/// The note an SDK section prints when its target emits no package manifest.
pub(crate) const NO_IDENTITY_NOTE: &str =
    "No sample call: this SDK target emits no package metadata, so it has no published import name.";

/// What a consumer imports and installs: what the SDK target's own emitted package manifest
/// declares, computed by the same function the manifest writer uses. `None` when the target emits
/// no manifest — a consumer's import path for an unpublished SDK depends on where they vendor it,
/// which no declaration states, so there is nothing to print.
///
/// # Errors
///
/// Returns the target's own configuration error for a module or package name it would reject.
pub fn consumer_identity(sdk: SiblingSdk<'_>) -> Result<Option<ConsumerIdentity>, CoreError> {
    let language = sdk.language();
    match sdk {
        SiblingSdk::Go(t) => {
            if !t.package_metadata {
                return Ok(None);
            }
            Ok(Some(ConsumerIdentity {
                import: t.module.clone(),
                qualifier: go_qualifier(&sdk_package(&t.module)?),
                install: t.module.clone(),
                language,
            }))
        }
        SiblingSdk::Python(t) => {
            if !t.package_metadata {
                return Ok(None);
            }
            let package = sdk_package(&t.module)?;
            Ok(Some(ConsumerIdentity {
                install: t.package_info.resolved_name(&package)?,
                import: package.clone(),
                qualifier: package,
                language,
            }))
        }
        SiblingSdk::TypeScript(t) => {
            if !t.effective_package_metadata() {
                return Ok(None);
            }
            let package = t.package_info.resolved_name(&sdk_package(&t.module)?)?;
            Ok(Some(ConsumerIdentity {
                import: package.clone(),
                qualifier: String::new(),
                install: package,
                language,
            }))
        }
    }
}

/// Every name a Go sample, its wrapper or the compile unit's harness binds or imports, plus Go's
/// predeclared identifiers. An SDK package clause spelled like one of them would be shadowed by it
/// (`client := client.NewClient(…)`), so the sample imports the SDK under an alias instead.
///
/// Only Go needs such a list: Go is the one language whose samples spell the SDK's package name as an
/// identifier. A TypeScript unit names the package only in its `import … from "<name>"` specifier and
/// imports nothing from it but `Client` and the error type. A Python unit names the package only in
/// `from <package> import …`; it imports a sample's models inside the function that runs the sample,
/// so no model shares the module namespace with the harness, and its file name carries an underscore,
/// which no package name does (`sdk_package`), so the package directory beside it cannot shadow it.
/// A Python package named after a standard-library module (`json`) remains unimportable — for a
/// consumer as much as for the unit — which is the SDK's name to change, not the sample's.
const GO_TAKEN_NAMES: &[&str] = &[
    // The sample's locals and imports, and the wrapper's parameters.
    "client",
    "result",
    "err",
    "ctx",
    "fmt",
    "time",
    "baseURL",
    "apiKey",
    "token",
    "username",
    "password",
    // The compile unit's harness: its imports, its declarations, and every local and parameter.
    "bytes",
    "context",
    "errors",
    "json",
    "io",
    "http",
    "os",
    "strings",
    "testing",
    "docsWireRecord",
    "docsWireTransport",
    "TestDocsWire",
    "transport",
    "t",
    "path",
    "payload",
    "record",
    "request",
    "header",
    "status",
    "contentType",
    "body",
    "index",
    "name",
    "text",
    "operation",
    "outcome",
    "apiErr",
    // Predeclared identifiers.
    "any",
    "append",
    "bool",
    "byte",
    "cap",
    "clear",
    "close",
    "comparable",
    "complex",
    "complex128",
    "complex64",
    "copy",
    "delete",
    "error",
    "false",
    "float32",
    "float64",
    "imag",
    "int",
    "int16",
    "int32",
    "int64",
    "int8",
    "iota",
    "len",
    "make",
    "max",
    "min",
    "new",
    "nil",
    "panic",
    "print",
    "println",
    "real",
    "recover",
    "rune",
    "string",
    "true",
    "uint",
    "uint16",
    "uint32",
    "uint64",
    "uint8",
    "uintptr",
];

/// The name a Go sample spells the SDK's symbols with: its package clause, or `<package>sdk` when
/// the clause is a name the sample already uses.
fn go_qualifier(package: &str) -> String {
    if GO_TAKEN_NAMES.contains(&package) {
        format!("{package}sdk")
    } else {
        package.to_string()
    }
}

/// The Go import entry for the SDK: its module path, aliased when the qualifier is not the package
/// clause (an entry with a space prints as `alias "path"`).
pub(crate) fn go_sdk_import(identity: &ConsumerIdentity) -> Result<String, CoreError> {
    Ok(if sdk_package(&identity.import)? == identity.qualifier {
        identity.import.clone()
    } else {
        format!("{} {}", identity.qualifier, identity.import)
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::consumer_identity;
    use crate::sdk::builtins::SiblingSdk;
    use crate::sdk::prelude::{GoSdk, PySdk, SdkPackageMetadata, TsSdk};

    /// A consumer installs exactly the name each SDK's package manifest declares, with the
    /// language's own tool: the Go module path, the Python distribution name (which may differ from
    /// the import package), the npm package name.
    #[test]
    fn each_identity_installs_what_its_manifest_declares() {
        let go = GoSdk::new().module("example.com/acme/client").to("go");
        let go = consumer_identity(SiblingSdk::Go(&go)).unwrap().unwrap();
        assert_eq!(go.import(), "example.com/acme/client");
        assert_eq!(go.qualifier, "clientsdk");
        assert_eq!(go.install_command(), "go get example.com/acme/client");

        let py = PySdk::new()
            .module("example.com/acme/acme")
            .package(SdkPackageMetadata::new().registry_name("acme-sdk"))
            .to("py");
        let py = consumer_identity(SiblingSdk::Python(&py)).unwrap().unwrap();
        assert_eq!(py.import(), "acme");
        assert_eq!(py.install(), "acme-sdk");
        assert_eq!(py.install_command(), "pip install acme-sdk");

        let ts = TsSdk::new()
            .module("acme")
            .package(SdkPackageMetadata::new().registry_name("@acme/sdk"))
            .to("ts");
        let ts = consumer_identity(SiblingSdk::TypeScript(&ts))
            .unwrap()
            .unwrap();
        assert_eq!(ts.import(), "@acme/sdk");
        assert_eq!(ts.install_command(), "npm install @acme/sdk");

        let bare = TsSdk::new().module("acme").to("ts");
        assert_eq!(
            consumer_identity(SiblingSdk::TypeScript(&bare)).unwrap(),
            None
        );
    }
}
