//! The client's own assets, in front of the component library's.
//!
//! The library carries the Lucide icon set, and every icon this client draws
//! comes from it. Two marks are ours: the brand mark the sign-in page draws the
//! way macOS's `BrandLogoView` does, and Google's, which the sign-in page shows
//! when the Server's identity provider is Google — the same asset and the same
//! notice the macOS client bundles (`assets/NOTICE.md`).
//!
//! Anything else falls through to the library, so a path this client does not
//! carry is the library's answer rather than a missing file.

use std::borrow::Cow;

use gpui_kit::*;

pub struct Assets;

/// Both marks travel inside the binary: a client that cannot find its own
/// sign-in page's mark has failed at the first screen.
const BRAND_MARK: &str = "brand/brand-mark.png";
const GOOGLE_MARK: &str = "brand/google-g.png";

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match path {
            BRAND_MARK => Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/brand-mark.png"
            )))),
            GOOGLE_MARK => Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/google-g.png"
            )))),
            _ => gpui_kit::assets::AllAssets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        gpui_kit::assets::AllAssets.list(path)
    }
}
