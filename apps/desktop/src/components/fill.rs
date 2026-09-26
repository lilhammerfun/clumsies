//! A box that fills its parent and reports the height it was laid out with.
//!
//! Why this exists: a percentage height resolves against a parent whose height
//! is a style value, and a column sized by flex is not one, so a child asking
//! for 100% inside one collapses to its own minimum. The text component's
//! editor element asks for exactly that, which is why an editor told to fill a
//! pane drew a single line and then a scrollbar. This box measures itself
//! instead: the size arrives with the first layout, the frame after it is built
//! with a number, and everything below uses pixels.
//!
//! It is a component rather than a line in the document pane because the next
//! pane-filling control (a review diff, a bundle list) has the same problem.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::*;

/// The box. Its content is built with the height reported on this frame,
/// which is zero until the first layout has run.
/// What to do when the box has a new height: whoever owns it redraws then.
type Measured = Box<dyn Fn(&mut App) + 'static>;

#[derive(IntoElement)]
pub struct Fill {
    height: Rc<Cell<Pixels>>,
    measured: Option<Measured>,
    content: Option<AnyElement>,
}

impl Fill {
    /// The cell the measurement is kept in belongs to whatever draws the box,
    /// so the number outlives a frame — and so the caller can read it while
    /// building the content that has to fit.
    pub fn new(height: Rc<Cell<Pixels>>) -> Self {
        Self {
            height,
            measured: None,
            content: None,
        }
    }

    /// Called when the measurement changes, which is the frame that first has
    /// the number. Whoever owns the box redraws there.
    pub fn on_measure(mut self, listener: impl Fn(&mut App) + 'static) -> Self {
        self.measured = Some(Box::new(listener));
        self
    }

    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.content = Some(child.into_any_element());
        self
    }
}

impl RenderOnce for Fill {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let height = self.height;
        let measured = self.measured;
        div()
            .relative()
            .flex_1()
            .min_h(px(0.))
            // The measurement is the box's own: the child is stretched over it,
            // so its bounds are the bounds the content has to work with.
            .on_children_prepainted(move |bounds, _window, cx| {
                let Some(bounds) = bounds.first() else {
                    return;
                };
                if height.get() == bounds.size.height {
                    return;
                }
                height.set(bounds.size.height);
                if let Some(listener) = &measured {
                    listener(cx);
                }
            })
            .child(div().absolute().inset_0().children(self.content))
    }
}
