//! SDK documentation output policy.
//!
//! The renderers that turn this policy into `README.md` / `reference.md` live in the host engine;
//! this is only the declaration a target carries.

/// SDK documentation output mode.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SdkDocs {
    reference: bool,
}

impl SdkDocs {
    /// Do not emit generated SDK documentation.
    #[must_use]
    pub fn none() -> Self {
        Self { reference: false }
    }

    /// Emit the SDK's `README.md` and `reference.md`, rendered from the docs model.
    #[must_use]
    pub fn reference() -> Self {
        Self { reference: true }
    }

    /// Whether no documentation should be emitted.
    #[must_use]
    pub const fn is_none(&self) -> bool {
        !self.reference
    }
}

impl Default for SdkDocs {
    fn default() -> Self {
        Self::reference()
    }
}

impl From<bool> for SdkDocs {
    fn from(enabled: bool) -> Self {
        if enabled {
            Self::reference()
        } else {
            Self::none()
        }
    }
}
