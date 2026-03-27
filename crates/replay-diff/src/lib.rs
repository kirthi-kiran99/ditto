pub mod diff;
pub mod manifest;
pub mod report;

pub use diff::{DiffCategory, DiffConfig, DiffEngine, DiffNode, DiffReport, NoiseRule, NoiseStrategy};
pub use manifest::{ChangeManifest, IntentionalChange};
pub use report::{InteractionDiffReport, InteractionMismatch};
