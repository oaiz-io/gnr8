//! Declared CLI command spec: taxonomy, compositions, views, and rename errors.
//!
//! These are facts about the generated program, not about the API. An operation still has one
//! canonical id; the spec says how that operation is invoked, confirmed, and displayed.

/// How strongly a command asks before it sends a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CliSeverity {
    /// No prompt.
    #[default]
    Mild,
    /// `[y/N]` naming the resource; `--yes` skips it.
    Moderate,
    /// Type the resource name; `--yes` skips it.
    Severe,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_mild(value: &CliSeverity) -> bool {
    matches!(value, CliSeverity::Mild)
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(value: &bool) -> bool {
    !*value
}

/// One root topic: a noun with optional concept text, a root-help section, and its commands.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CliTopic {
    /// Invocation name (`workflow`, `books`).
    pub name: String,
    /// What this topic is, printed at the top of its help page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concept: Option<String>,
    /// Root-help section heading this topic is listed under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    /// Commands under this topic, in declaration order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<CliCommand>,
}

impl CliTopic {
    /// A topic invoked as `name`.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            concept: None,
            section: None,
            commands: Vec::new(),
        }
    }

    /// The paragraph its help page opens with.
    #[must_use]
    pub fn concept(mut self, concept: impl Into<String>) -> Self {
        self.concept = Some(concept.into());
        self
    }

    /// The root-help section this topic is listed under.
    #[must_use]
    pub fn section(mut self, section: impl Into<String>) -> Self {
        self.section = Some(section.into());
        self
    }

    /// Add one command under this topic.
    #[must_use]
    pub fn command(mut self, command: CliCommand) -> Self {
        self.commands.push(command);
        self
    }
}

/// One declared command: the operation it wraps and how it is invoked.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CliCommand {
    /// Graph operation id this command wraps.
    pub operation: String,
    /// Verb token (`list`, `get`, `delete`).
    pub verb: String,
    /// Optional sub-noun between the topic and the verb (`access`, `version`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub_noun: Option<String>,
    /// Identifier parameters taken positionally, in order, by graph parameter name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub positionals: Vec<String>,
    /// Confirmation strength. Mild is the default and is not serialized.
    #[serde(default, skip_serializing_if = "is_mild")]
    pub severity: CliSeverity,
    /// Emit scalar body fields as flags that override the same key in `--body`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub body_fields: bool,
    /// JSON object sent as the request body, replacing `--body` / `--body-file`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_body: Option<String>,
    /// A flag that, when set, calls a different operation instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub switch_flag: Option<CliSwitchFlag>,
    /// `@v12` / `@latest` selector resolved with one list call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<CliSelector>,
}

impl CliCommand {
    /// A command that wraps `operation` and is invoked as `verb`.
    #[must_use]
    pub fn operation(operation: impl Into<String>, verb: impl Into<String>) -> Self {
        Self {
            operation: operation.into(),
            verb: verb.into(),
            sub_noun: None,
            positionals: Vec::new(),
            severity: CliSeverity::Mild,
            body_fields: false,
            fixed_body: None,
            switch_flag: None,
            selector: None,
        }
    }

    /// Place this command under a sub-noun (`workflow access grant`).
    #[must_use]
    pub fn sub_noun(mut self, name: impl Into<String>) -> Self {
        self.sub_noun = Some(name.into());
        self
    }

    /// Take this graph parameter as a positional identifier.
    #[must_use]
    pub fn positional(mut self, param: impl Into<String>) -> Self {
        self.positionals.push(param.into());
        self
    }

    /// Confirmation strength for this command.
    #[must_use]
    pub fn severity(mut self, severity: CliSeverity) -> Self {
        self.severity = severity;
        self
    }

    /// Generate flags from scalar request-body fields.
    #[must_use]
    pub fn body_fields(mut self) -> Self {
        self.body_fields = true;
        self
    }

    /// Send this JSON object as the body; `--body` is not bound.
    #[must_use]
    pub fn fixed_body(mut self, json: impl Into<String>) -> Self {
        self.fixed_body = Some(json.into());
        self
    }

    /// When `flag` is set, call `operation` instead.
    #[must_use]
    pub fn switch_flag(mut self, flag: impl Into<String>, operation: impl Into<String>) -> Self {
        self.switch_flag = Some(CliSwitchFlag {
            flag: flag.into(),
            operation: operation.into(),
        });
        self
    }

    /// Resolve `@vN` / `@latest` on the first positional via one list call.
    #[must_use]
    pub fn selector(mut self, selector: CliSelector) -> Self {
        self.selector = Some(selector);
        self
    }
}

/// A boolean flag that selects a different operation for the same verb.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CliSwitchFlag {
    /// Flag name without the `--` prefix (`content`, `deleted`).
    pub flag: String,
    /// Operation id to call when the flag is set.
    pub operation: String,
}

/// Resolve a `<id>@v12` / `<id>@latest` token with one list operation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CliSelector {
    /// Operation id of the list call that returns candidate versions.
    pub list_operation: String,
    /// Field on each list item holding the version number (`version`).
    pub match_field: String,
    /// Field on each list item holding the id to send (`id`).
    pub id_field: String,
}

impl CliSelector {
    /// A selector that lists `list_operation` and matches `match_field` to fill `id_field`.
    #[must_use]
    pub fn new(
        list_operation: impl Into<String>,
        match_field: impl Into<String>,
        id_field: impl Into<String>,
    ) -> Self {
        Self {
            list_operation: list_operation.into(),
            match_field: match_field.into(),
            id_field: id_field.into(),
        }
    }
}

/// A retired invocation that names its replacement and exits 2 without sending a request.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CliRenameError {
    /// Old tokens, including the topic (`books list-books`).
    pub from: Vec<String>,
    /// Exact replacement the user should type (`books list`).
    pub to: String,
}

impl CliRenameError {
    /// `from` is the retired path; `to` is the exact replacement.
    #[must_use]
    pub fn new(from: impl IntoIterator<Item = impl Into<String>>, to: impl Into<String>) -> Self {
        Self {
            from: from.into_iter().map(Into::into).collect(),
            to: to.into(),
        }
    }
}

/// Preview and table fields for one response schema. Required for list item types.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CliView {
    /// Graph schema id this view describes.
    pub schema: String,
    /// Fields shown in the ai-friendly preview row.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preview: Vec<String>,
    /// Columns shown in a human table.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub table: Vec<String>,
}

impl CliView {
    /// A view for `schema`.
    #[must_use]
    pub fn schema(schema: impl Into<String>) -> Self {
        Self {
            schema: schema.into(),
            preview: Vec::new(),
            table: Vec::new(),
        }
    }

    /// Fields shown in the ai-friendly preview row.
    #[must_use]
    pub fn preview<I, S>(mut self, fields: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.preview = fields.into_iter().map(Into::into).collect();
        self
    }

    /// Columns shown in a human table.
    #[must_use]
    pub fn table<I, S>(mut self, fields: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.table = fields.into_iter().map(Into::into).collect();
        self
    }
}
