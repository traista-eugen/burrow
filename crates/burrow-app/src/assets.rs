//! The app's assets: gpui-component's default icons, plus the few Lucide icons the app uses
//! beyond them.
//!
//! `gpui_kit_assets::Assets` embeds only gpui-component's own icons; any other Lucide icon
//! draws as nothing unless an asset source provides it. [`ExtraIcons`] embeds just the ones
//! listed, so the binary doesn't carry the whole catalogue. Add an icon here when a view uses a
//! new one; the test below checks the list covers what the views name.

use std::borrow::Cow;

use gpui::{AssetSource, SharedString};

gpui_kit_assets::icon_assets!(
    ExtraIcons,
    [Archive, CircleStop, Download, Hourglass, Pencil]
);

/// Every asset the app loads.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = ExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit_assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        let mut paths = gpui_kit_assets::Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit_assets::IconName;

    use super::*;

    #[test]
    fn the_icons_views_use_are_embedded() {
        // The Lucide icons the views name beyond gpui-component's set, and a few of its own.
        for icon in [
            IconName::Archive,
            IconName::CircleStop,
            IconName::Download,
            IconName::Hourglass,
            IconName::Pencil,
            IconName::FolderOpen,
            IconName::Inbox,
        ] {
            let path = icon.path();
            assert!(
                Assets.load(&path).unwrap().is_some(),
                "{path} isn't embedded"
            );
        }
        // Not everything is: the catalogue stays out of the binary.
        assert!(!matches!(Assets.load(&IconName::Bike.path()), Ok(Some(_))));
    }
}
