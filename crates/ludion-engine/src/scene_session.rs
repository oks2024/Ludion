//! The scene-session sidecar: what `scene.save`'s conflict check needs to survive a dev restart.
//!
//! The child writes it when it answers `engine.snapshot`, because that is when the world is
//! captured: the frame, the scene path and the conflict bases then describe one moment. The next
//! `--restore` boot adopts it only if its frame equals the restored frame.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub(crate) struct SceneSessionDoc {
    pub(crate) frame: u64,
    pub(crate) path: Option<String>,
    /// Absolute key path → the text the session last loaded or saved there.
    pub(crate) bases: BTreeMap<String, String>,
}

impl SceneSessionDoc {
    pub(crate) fn write(&self, file: &Path) -> std::io::Result<()> {
        let text = serde_json::to_string(self).map_err(std::io::Error::other)?;
        ludion_core::atomic::write_atomic(file, text)
    }

    pub(crate) fn read(file: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(file).map_err(|error| error.to_string())?;
        serde_json::from_str(&text).map_err(|error| format!("not a scene session: {error}"))
    }
}
