//! The brand logos drawn beside the host and editor names in the help
//! menu's switcher. The logos ship with the binary as small PNGs and are
//! drawn with the terminal's own image protocol (Kitty, iTerm2, or Sixel),
//! so no icon font need be installed.
//!
//! A terminal with no image protocol falls back to half-blocks, which at a
//! two-cell icon size is an unreadable smudge; there [`Icons::load`] returns
//! None and the switcher shows a coloured dot instead.

use crate::config::Ide;
use crate::forge::Forge;
use ratatui::{Frame, layout::Rect};
use ratatui_image::{
    Resize, StatefulImage, picker::Picker, picker::ProtocolType, protocol::StatefulProtocol,
};

// The logos, each a 64×64 RGBA PNG from the project's own brand assets.
const GITHUB: &[u8] = include_bytes!("../assets/icons/github.png");
const GITLAB: &[u8] = include_bytes!("../assets/icons/gitlab.png");
const ZED: &[u8] = include_bytes!("../assets/icons/zed.png");
const VSCODE: &[u8] = include_bytes!("../assets/icons/vscode.png");
const CURSOR: &[u8] = include_bytes!("../assets/icons/cursor.png");

/// One decoded logo per host and editor, each ready to draw through the
/// terminal's image protocol.
pub struct Icons {
    github: StatefulProtocol,
    gitlab: StatefulProtocol,
    zed: StatefulProtocol,
    vscode: StatefulProtocol,
    cursor: StatefulProtocol,
}

impl Icons {
    /// Asks the terminal which image protocol it speaks and decodes the
    /// logos for it, or None when the terminal cannot draw an image worth
    /// showing at icon size, so the caller can fall back to coloured dots.
    ///
    /// Must run after the alternate screen is entered and before events are
    /// read, since it writes a query to the terminal and reads the reply.
    pub fn load() -> Option<Self> {
        let picker = Picker::from_query_stdio().ok()?;
        // Half-blocks are all this terminal has; a two-cell logo drawn in
        // them is a blur, so the dot reads better. Real protocols go on.
        if matches!(picker.protocol_type(), ProtocolType::Halfblocks) {
            return None;
        }
        let decode = |bytes| image::load_from_memory(bytes).ok();
        Some(Self {
            github: picker.new_resize_protocol(decode(GITHUB)?),
            gitlab: picker.new_resize_protocol(decode(GITLAB)?),
            zed: picker.new_resize_protocol(decode(ZED)?),
            vscode: picker.new_resize_protocol(decode(VSCODE)?),
            cursor: picker.new_resize_protocol(decode(CURSOR)?),
        })
    }

    fn forge_mut(&mut self, forge: Forge) -> &mut StatefulProtocol {
        match forge {
            Forge::Gitlab => &mut self.gitlab,
            Forge::Github => &mut self.github,
        }
    }

    fn ide_mut(&mut self, ide: Ide) -> &mut StatefulProtocol {
        match ide {
            Ide::Zed => &mut self.zed,
            Ide::VsCode => &mut self.vscode,
            Ide::Cursor => &mut self.cursor,
        }
    }

    /// Draws a host's logo in `area`, keeping its shape.
    pub fn draw_forge(&mut self, frame: &mut Frame, area: Rect, forge: Forge) {
        let image = StatefulImage::default().resize(Resize::Fit(None));
        frame.render_stateful_widget(image, area, self.forge_mut(forge));
    }

    /// Draws an editor's logo in `area`, keeping its shape.
    pub fn draw_ide(&mut self, frame: &mut Frame, area: Rect, ide: Ide) {
        let image = StatefulImage::default().resize(Resize::Fit(None));
        frame.render_stateful_widget(image, area, self.ide_mut(ide));
    }
}
