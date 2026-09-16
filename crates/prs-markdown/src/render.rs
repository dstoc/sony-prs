//! Generic rendering boundary for a paginated display list.

use crate::pagination::PageLayout;
use embedded_graphics::draw_target::DrawTarget;

/// Early architecture-stage renderer adapter.
///
/// The display list is intentionally independent of pixels and fonts.  This
/// adapter proves that a page can be handed to any `embedded-graphics`
/// draw target; typography and image decoding will supply actual draw commands
/// in later issues.
#[derive(Clone, Copy, Debug, Default)]
pub struct EmbeddedGraphicsRenderer;

impl EmbeddedGraphicsRenderer {
    pub fn render<T: DrawTarget>(&self, page: &PageLayout, target: &mut T) -> Result<(), T::Error> {
        let _ = (page, target);
        Ok(())
    }
}
