//! Markdown rendering for Memory documents.
//!
//! Frontmatter is not part of CommonMark, so the parser has to be told to read
//! it and a plugin has to render the resulting node as the description list
//! the macOS client shows.

use gpui_kit::component::text::{FrontmatterPlugin, MarkdownExtensions, TextView};
use gpui_kit::*;

pub fn memory_document(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
) -> impl IntoElement {
    TextView::markdown(id, text)
        .markdown_extensions(MarkdownExtensions::default().frontmatter())
        .plugin(FrontmatterPlugin::new())
        .selectable(true)
        .scrollable(true)
        .size_full()
}
