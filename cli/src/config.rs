//! What onboarding asked for, kept between runs: which host to assume for
//! a clone whose remote does not name one, and which editor to hand a
//! review to.

use crate::forge::Forge;
use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The editors a review can be opened in. Zed shows one through its fork;
/// VS Code and Cursor share one extension (`editors/vscode`), launched by
/// the same review URL under each editor's own scheme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ide {
    #[default]
    Zed,
    VsCode,
    Cursor,
}

impl Ide {
    pub const ALL: [Ide; 3] = [Ide::Zed, Ide::VsCode, Ide::Cursor];

    pub fn name(self) -> &'static str {
        match self {
            Ide::Zed => "Zed",
            Ide::VsCode => "VS Code",
            Ide::Cursor => "Cursor",
        }
    }

    /// Why someone would pick it, for the onboarding list.
    pub fn description(self) -> &'static str {
        match self {
            Ide::Zed => "opens the diff in review order, with notes and threads",
            Ide::VsCode => "opens the diff through the changewalk extension",
            Ide::Cursor => "opens the diff through the changewalk extension",
        }
    }

    /// The URL scheme the editor registers, which `open` hands the review
    /// URL to. The VS Code extension handles whichever of `vscode` and
    /// `cursor` launched it.
    pub fn url_scheme(self) -> &'static str {
        match self {
            Ide::Zed => "zed",
            Ide::VsCode => "vscode",
            Ide::Cursor => "cursor",
        }
    }

    /// The path after the scheme for a review URL. Zed has a built-in
    /// `git/review` handler; VS Code and Cursor route to the extension by
    /// its `publisher.name`.
    pub fn review_path(self) -> &'static str {
        match self {
            Ide::Zed => "git/review",
            Ide::VsCode | Ide::Cursor => "changewalk.changewalk/review",
        }
    }

    /// Zed runs the review inside the editor and so wants the model key and
    /// its own order cache; VS Code and Cursor are thin clients the CLI
    /// feeds, so they want neither.
    pub fn runs_review_in_editor(self) -> bool {
        matches!(self, Ide::Zed)
    }

    /// Builds a `scheme://path?query` review URL, each value percent-encoded.
    pub fn review_url(self, query: &[(&str, String)]) -> String {
        let query = query
            .iter()
            .map(|(key, value)| format!("{key}={}", urlencoding::encode(value)))
            .collect::<Vec<_>>()
            .join("&");
        format!("{}://{}?{query}", self.url_scheme(), self.review_path())
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
            Ide::VsCode => &[
                "/Applications/Visual Studio Code.app",
                "~/Applications/Visual Studio Code.app",
            ],
            Ide::Cursor => &["/Applications/Cursor.app", "~/Applications/Cursor.app"],
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

    /// Deletes the saved configuration so the next run onboards afresh, and
    /// removes the `changewalk` directory if nothing else is left in it.
    /// Returns the file removed, or None when there was none to remove.
    pub fn reset() -> Result<Option<PathBuf>> {
        let path = Self::path().context("no configuration directory for this user")?;
        Self::reset_at(&path)
    }

    fn reset_at(path: &Path) -> Result<Option<PathBuf>> {
        if !path.exists() {
            return Ok(None);
        }
        std::fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?;
        if let Some(dir) = path.parent() {
            // Only if empty: a future version might keep more here.
            std::fs::remove_dir(dir).ok();
        }
        Ok(Some(path.to_path_buf()))
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

    /// Reset removes the file and the directory it was alone in, and says
    /// nothing was there when run a second time.
    #[test]
    fn reset_forgets_the_saved_configuration() {
        let dir = std::env::temp_dir().join(format!("changewalk-reset-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let file = dir.join("changewalk").join("config.json");

        // Nothing saved: nothing to forget, and no error.
        assert_eq!(Config::reset_at(&file).unwrap(), None);

        Config::default().save_to(&file).unwrap();
        assert!(file.exists());
        assert_eq!(Config::reset_at(&file).unwrap(), Some(file.clone()));
        assert!(!file.exists());
        // The empty parent went too.
        assert!(!file.parent().unwrap().exists());
        // A second reset is a quiet no-op.
        assert_eq!(Config::reset_at(&file).unwrap(), None);

        std::fs::remove_dir_all(&dir).ok();
    }
}
