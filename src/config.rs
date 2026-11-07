//! What onboarding asked for, kept between runs: which host to assume for
//! a clone whose remote does not name one, and which editor to hand a
//! review to.

use crate::forge::Forge;
use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The editors a review can be opened in. Only Zed knows how to show one
/// so far; the choice is stored so adding another is a matter of listing
/// it here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ide {
    #[default]
    Zed,
}

impl Ide {
    pub const ALL: [Ide; 1] = [Ide::Zed];

    pub fn name(self) -> &'static str {
        match self {
            Ide::Zed => "Zed",
        }
    }

    /// Why someone would pick it, for the onboarding list.
    pub fn description(self) -> &'static str {
        match self {
            Ide::Zed => "opens the diff in review order, with notes and threads",
        }
    }

    /// Where the app usually is, most specific build first, so a
    /// development build is preferred over a released one.
    pub fn app_candidates(self) -> &'static [&'static str] {
        match self {
            Ide::Zed => &[
                "~/Applications/Zed Dev.app",
                "/Applications/Zed Dev.app",
                "~/Applications/Zed.app",
                "/Applications/Zed.app",
            ],
        }
    }

    /// The app bundle to open, the first candidate that is installed, or
    /// the last candidate so the error names a real path.
    pub fn default_app(self) -> PathBuf {
        let candidates = self.app_candidates();
        candidates
            .iter()
            .map(|path| crate::expand_home(path))
            .find(|path| path.exists())
            .unwrap_or_else(|| crate::expand_home(candidates[candidates.len() - 1]))
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Config {
    /// The host to assume for a clone whose remote does not name one, as a
    /// self-hosted GitLab or GitHub Enterprise does not.
    #[serde(default)]
    pub forge: Forge,
    #[serde(default)]
    pub ide: Ide,
    /// The app bundle to open, chosen during onboarding; `--zed-app` and
    /// `ZED_APP` still win over it.
    #[serde(default)]
    pub app: Option<PathBuf>,
}

impl Config {
    /// `$XDG_CONFIG_HOME/changewalk/config.json`, else
    /// `~/.config/changewalk/config.json`. Not `dirs::config_dir`, which on
    /// macOS is `~/Library/Application Support`: this is a terminal tool,
    /// and `glab` and `gh` keep their own logins under `~/.config`.
    pub fn path() -> Option<PathBuf> {
        let dir = match std::env::var_os("XDG_CONFIG_HOME") {
            Some(dir) if !dir.is_empty() => PathBuf::from(dir),
            _ => dirs::home_dir()?.join(".config"),
        };
        Some(dir.join("changewalk").join("config.json"))
    }

    /// The saved configuration, or None when onboarding has not run.
    pub fn load() -> Option<Self> {
        let text = std::fs::read_to_string(Self::path()?).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save(&self) -> Result<PathBuf> {
        let path = Self::path().context("no configuration directory for this user")?;
        self.save_to(&path)?;
        Ok(path)
    }

    fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(path, format!("{text}\n"))
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    /// The app bundle to open a review in.
    pub fn app(&self) -> PathBuf {
        self.app.clone().unwrap_or_else(|| self.ide.default_app())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The file is where a terminal tool's configuration belongs, and what
    /// is written there reads back the same.
    #[test]
    fn the_configuration_round_trips_under_dot_config() {
        let path = Config::path().unwrap();
        assert!(
            path.ends_with(".config/changewalk/config.json")
                || std::env::var_os("XDG_CONFIG_HOME").is_some(),
            "{}",
            path.display()
        );
        let config = Config {
            forge: Forge::Github,
            ide: Ide::Zed,
            app: Some(PathBuf::from("/Applications/Zed.app")),
        };
        let text = serde_json::to_string_pretty(&config).unwrap();
        assert!(text.contains("\"github\""), "{text}");
        assert!(text.contains("\"zed\""), "{text}");
        let read: Config = serde_json::from_str(&text).unwrap();
        assert_eq!(read.forge, Forge::Github);
        assert_eq!(read.ide, Ide::Zed);
        assert_eq!(read.app, config.app);

        // A file from an older version, or a hand-written one, still reads.
        let sparse: Config = serde_json::from_str("{\"forge\":\"gitlab\"}").unwrap();
        assert_eq!(sparse.forge, Forge::Gitlab);
        assert_eq!(sparse.ide, Ide::Zed);
        assert_eq!(sparse.app, None);

        // And it reaches the disk, directories and all.
        let dir = std::env::temp_dir().join(format!("changewalk-config-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let file = dir.join("changewalk").join("config.json");
        config.save_to(&file).unwrap();
        let written = std::fs::read_to_string(&file).unwrap();
        assert!(written.ends_with("\n"), "{written:?}");
        let read: Config = serde_json::from_str(&written).unwrap();
        assert_eq!(read.forge, Forge::Github);
        assert_eq!(read.app, config.app);
        std::fs::remove_dir_all(&dir).ok();
    }
}
