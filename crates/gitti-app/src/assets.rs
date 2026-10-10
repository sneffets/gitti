//! The app's asset source: gpui-kit's bundled component icons plus the few extra Lucide
//! icons gitti draws itself (only those are embedded, not the whole catalog).

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

gpui_kit::assets::icon_assets!(ExtraIcons, [KeyRound, Funnel, FolderGit2]);

/// Key of the credential prompt (askpass dialog): an SVG, so it looks the same on every
/// platform (an emoji depends on the system fonts — Windows showed none).
pub const KEY_ICON: gpui_kit::assets::IconName = gpui_kit::assets::IconName::KeyRound;

/// Mark of a sparse worktree (instead of the word "sparse"): only some folders pass — a
/// funnel, an SVG for the same reason as the key.
pub const SPARSE_ICON: gpui_kit::assets::IconName = gpui_kit::assets::IconName::Funnel;

/// Mark of a subtree in the sidebar: another repository's files in a folder.
pub const SUBTREE_ICON: gpui_kit::assets::IconName = gpui_kit::assets::IconName::FolderGit2;

pub struct GittiAssets;

impl AssetSource for GittiAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = ExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::assets::IconNamed as _;

    #[gitti_test::test]
    fn key_icon_is_embedded_next_to_the_component_icons() {
        let key = KEY_ICON.path();
        assert!(GittiAssets.load(&key).unwrap().is_some(), "{key}");
        let sparse = SPARSE_ICON.path();
        assert!(GittiAssets.load(&sparse).unwrap().is_some(), "{sparse}");
        let folder = gpui_kit::component::IconName::Folder.path();
        assert!(GittiAssets.load(&folder).unwrap().is_some(), "{folder}");
    }
}
