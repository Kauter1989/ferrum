//! Interactive 2D tools as UI-independent state machines.
//!
//! The presentation layer converts raw pointer input into [`ToolInput`]
//! (with the pointer position already mapped to in-plane millimetres) and
//! forwards it to [`ToolController::handle`]. The controller mutates the
//! slice view, window or annotation set through a [`SliceContext`] and
//! reports what happened as a [`ToolOutcome`].

use ferrum_domain::{
    Annotation, AnnotationId, AnnotationSet, IntensityRange, SliceAxis, SliceKey, SliceView, Volume, WindowLevel,
};
use glam::{UVec3, Vec2};

/// Available 2D tools (the toolbar of the original viewer, minus
/// segmentation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ToolKind {
    /// Drag to pan, wheel to zoom.
    #[default]
    Pan,
    /// Drag to change window centre/width.
    WindowLevel,
    /// Show the voxel value under the pointer.
    Probe,
    /// Measure a distance.
    Distance,
    /// Measure an angle (three clicks).
    Angle,
    /// Measure a free-form area (click vertices, double-click to close).
    Area,
    /// Measure a rectangular area.
    Rect,
    /// Place a text note.
    Text,
    /// Move annotations.
    Move,
    /// Delete annotations.
    Delete,
}

impl ToolKind {
    /// All tools in toolbar order.
    pub const ALL: [ToolKind; 10] = [
        ToolKind::Pan,
        ToolKind::WindowLevel,
        ToolKind::Probe,
        ToolKind::Distance,
        ToolKind::Angle,
        ToolKind::Area,
        ToolKind::Rect,
        ToolKind::Text,
        ToolKind::Move,
        ToolKind::Delete,
    ];

    /// Short label.
    pub fn label(&self) -> &'static str {
        match self {
            ToolKind::Pan => "Pan / zoom",
            ToolKind::WindowLevel => "Window / level",
            ToolKind::Probe => "Voxel value",
            ToolKind::Distance => "Distance",
            ToolKind::Angle => "Angle",
            ToolKind::Area => "Area",
            ToolKind::Rect => "Rectangle",
            ToolKind::Text => "Text",
            ToolKind::Move => "Move",
            ToolKind::Delete => "Delete",
        }
    }

    /// Tooltip text.
    pub fn hint(&self) -> &'static str {
        match self {
            ToolKind::Pan => "Drag to pan, scroll to zoom",
            ToolKind::WindowLevel => "Drag horizontally for width, vertically for level",
            ToolKind::Probe => "Hover to read the voxel intensity",
            ToolKind::Distance => "Drag to measure a distance",
            ToolKind::Angle => "Click three points: arm, vertex, arm",
            ToolKind::Area => "Click vertices, double-click or click the first vertex to close",
            ToolKind::Rect => "Drag to measure a rectangular area",
            ToolKind::Text => "Click to place a note",
            ToolKind::Move => "Drag an annotation to move it",
            ToolKind::Delete => "Click an annotation to delete it",
        }
    }
}

/// Kind of pointer input.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputKind {
    /// Primary button pressed.
    Press,
    /// Pointer moved with the primary button held; `delta` in screen points.
    Drag {
        /// Movement since the previous event, in screen points.
        delta: Vec2,
    },
    /// Primary button released.
    Release,
    /// Pointer moved without buttons.
    Hover,
    /// Double click.
    DoubleClick,
    /// Scroll; positive zooms in.
    Scroll {
        /// Scroll amount in "notches".
        amount: f32,
    },
}

/// One pointer event in slice coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToolInput {
    /// Event kind.
    pub kind: InputKind,
    /// Pointer position in in-plane millimetres.
    pub mm: Vec2,
    /// Pointer position in viewport points.
    pub screen: Vec2,
}

/// Value under the probe.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProbeReading {
    /// Voxel index.
    pub voxel: UVec3,
    /// Physical value.
    pub value: f32,
}

/// Everything a tool may read or modify.
pub struct SliceContext<'a> {
    /// Slice being interacted with.
    pub key: SliceKey,
    /// Orientation.
    pub axis: SliceAxis,
    /// Slice index.
    pub index: u32,
    /// Volume shown.
    pub volume: &'a Volume,
    /// Pan / zoom state.
    pub view: &'a mut SliceView,
    /// Viewport size in points.
    pub viewport: Vec2,
    /// Display window.
    pub window: &'a mut WindowLevel,
    /// Annotations.
    pub annotations: &'a mut AnnotationSet,
    /// Hit-test tolerance in millimetres.
    pub tolerance_mm: f32,
}

impl SliceContext<'_> {
    fn image_mm(&self) -> Vec2 {
        self.axis.plane_size_mm(self.volume)
    }

    fn range(&self) -> IntensityRange {
        self.volume.range()
    }

    /// Voxel under in-plane position `mm`, if inside the image.
    pub fn probe(&self, mm: Vec2) -> Option<ProbeReading> {
        let (w, h) = self.axis.plane_dims(self.volume);
        let uv = mm / self.image_mm();
        if uv.x < 0.0 || uv.y < 0.0 || uv.x >= 1.0 || uv.y >= 1.0 {
            return None;
        }
        let px = ((uv.x * w as f32) as u32).min(w - 1);
        let py = ((uv.y * h as f32) as u32).min(h - 1);
        let voxel = self.axis.voxel_at(self.volume, self.index, px, py)?;
        let value = self.volume.physical(voxel.x, voxel.y, voxel.z)?;
        Some(ProbeReading { voxel, value })
    }
}

/// Result of handling an input event.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolOutcome {
    /// Nothing changed.
    None,
    /// State changed; redraw.
    Changed,
    /// An annotation was committed.
    Committed(AnnotationId),
    /// The user wants to place text at this position; the UI should ask
    /// for the text and call [`ToolController::commit_text`].
    RequestText(Vec2),
    /// New probe reading (or `None` when outside the image).
    Probe(Option<ProbeReading>),
}

#[derive(Debug, Clone, PartialEq, Default)]
enum Draft {
    #[default]
    Idle,
    Line(Vec2, Vec2),
    Points(Vec<Vec2>),
    Rect(Vec2, Vec2),
    Moving(AnnotationId, Vec2),
}

/// State machine driving the active 2D tool.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolController {
    draft: Draft,
    hover: Option<Vec2>,
}

/// Minimum length (mm) for a distance/rectangle to be committed.
const MIN_SIZE_MM: f32 = 0.5;

impl ToolController {
    /// Aborts the current drawing.
    pub fn cancel(&mut self) {
        self.draft = Draft::Idle;
    }

    /// `true` while an annotation is being drawn or moved.
    pub fn is_busy(&self) -> bool {
        self.draft != Draft::Idle
    }

    /// Commits a text annotation requested via [`ToolOutcome::RequestText`].
    pub fn commit_text(
        &mut self,
        set: &mut AnnotationSet,
        key: SliceKey,
        pos: Vec2,
        text: &str,
    ) -> Option<AnnotationId> {
        let text = text.trim();
        (!text.is_empty()).then(|| set.add(key, Annotation::Text { pos, text: text.to_string() }))
    }

    /// Annotation being drawn, for preview rendering.
    pub fn preview(&self, tool: ToolKind) -> Option<Annotation> {
        match (&self.draft, tool) {
            (Draft::Line(a, b), _) => Some(Annotation::Distance { a: *a, b: *b }),
            (Draft::Rect(a, b), _) => Some(Annotation::Rect { a: *a, b: *b }),
            (Draft::Points(p), ToolKind::Angle) => {
                let mut pts = p.clone();
                pts.extend(self.hover);
                match pts.len() {
                    2 => Some(Annotation::Distance { a: pts[0], b: pts[1] }),
                    n if n >= 3 => Some(Annotation::Angle { a: pts[0], vertex: pts[1], b: pts[2] }),
                    _ => None,
                }
            }
            (Draft::Points(p), ToolKind::Area) => {
                let mut pts = p.clone();
                pts.extend(self.hover);
                (pts.len() >= 2).then_some(Annotation::Polygon { points: pts })
            }
            _ => None,
        }
    }

    /// Handles one input event for `tool`.
    pub fn handle(&mut self, tool: ToolKind, input: ToolInput, ctx: &mut SliceContext<'_>) -> ToolOutcome {
        if let InputKind::Scroll { amount } = input.kind {
            let image_mm = ctx.image_mm();
            ctx.view.zoom_about(1.15f32.powf(amount), input.screen, ctx.viewport, image_mm);
            return ToolOutcome::Changed;
        }
        if matches!(input.kind, InputKind::Hover | InputKind::Drag { .. }) {
            self.hover = Some(input.mm);
        }
        match tool {
            ToolKind::Pan => match input.kind {
                InputKind::Drag { delta } => {
                    ctx.view.pan += delta;
                    ToolOutcome::Changed
                }
                InputKind::DoubleClick => {
                    *ctx.view = SliceView::default();
                    ToolOutcome::Changed
                }
                _ => ToolOutcome::None,
            },
            ToolKind::WindowLevel => match input.kind {
                InputKind::Drag { delta } => {
                    let d = delta / ctx.viewport.max(Vec2::ONE);
                    *ctx.window = ctx.window.dragged(d.x, d.y, ctx.range());
                    ToolOutcome::Changed
                }
                _ => ToolOutcome::None,
            },
            ToolKind::Probe => match input.kind {
                InputKind::Hover | InputKind::Press | InputKind::Drag { .. } => ToolOutcome::Probe(ctx.probe(input.mm)),
                _ => ToolOutcome::None,
            },
            ToolKind::Distance | ToolKind::Rect => self.handle_two_point(tool, input, ctx),
            ToolKind::Angle => self.handle_angle(input, ctx),
            ToolKind::Area => self.handle_area(input, ctx),
            ToolKind::Text => match input.kind {
                InputKind::Press => ToolOutcome::RequestText(input.mm),
                _ => ToolOutcome::None,
            },
            ToolKind::Move => self.handle_move(input, ctx),
            ToolKind::Delete => match input.kind {
                InputKind::Press => match ctx.annotations.hit_test(ctx.key, input.mm, ctx.tolerance_mm) {
                    Some(id) => {
                        ctx.annotations.remove(id);
                        ToolOutcome::Changed
                    }
                    None => ToolOutcome::None,
                },
                _ => ToolOutcome::None,
            },
        }
    }

    fn handle_two_point(&mut self, tool: ToolKind, input: ToolInput, ctx: &mut SliceContext<'_>) -> ToolOutcome {
        let make = |a, b| if tool == ToolKind::Rect { Draft::Rect(a, b) } else { Draft::Line(a, b) };
        match (input.kind, &self.draft) {
            (InputKind::Press, _) => {
                self.draft = make(input.mm, input.mm);
                ToolOutcome::Changed
            }
            (InputKind::Drag { .. }, Draft::Line(a, _) | Draft::Rect(a, _)) => {
                self.draft = make(*a, input.mm);
                ToolOutcome::Changed
            }
            (InputKind::Release, Draft::Line(a, _) | Draft::Rect(a, _)) => {
                let (a, b) = (*a, input.mm);
                self.draft = Draft::Idle;
                let big_enough = if tool == ToolKind::Rect {
                    (b - a).abs().min_element() >= MIN_SIZE_MM
                } else {
                    a.distance(b) >= MIN_SIZE_MM
                };
                if !big_enough {
                    return ToolOutcome::Changed;
                }
                let ann =
                    if tool == ToolKind::Rect { Annotation::Rect { a, b } } else { Annotation::Distance { a, b } };
                ToolOutcome::Committed(ctx.annotations.add(ctx.key, ann))
            }
            _ => ToolOutcome::None,
        }
    }

    fn handle_angle(&mut self, input: ToolInput, ctx: &mut SliceContext<'_>) -> ToolOutcome {
        if input.kind != InputKind::Press {
            return if matches!(self.draft, Draft::Points(_)) { ToolOutcome::Changed } else { ToolOutcome::None };
        }
        let mut pts = match std::mem::take(&mut self.draft) {
            Draft::Points(p) => p,
            _ => Vec::new(),
        };
        pts.push(input.mm);
        if pts.len() == 3 {
            let id = ctx.annotations.add(ctx.key, Annotation::Angle { a: pts[0], vertex: pts[1], b: pts[2] });
            return ToolOutcome::Committed(id);
        }
        self.draft = Draft::Points(pts);
        ToolOutcome::Changed
    }

    fn handle_area(&mut self, input: ToolInput, ctx: &mut SliceContext<'_>) -> ToolOutcome {
        let mut pts = match std::mem::take(&mut self.draft) {
            Draft::Points(p) => p,
            _ => Vec::new(),
        };
        let close = |pts: Vec<Vec2>, ctx: &mut SliceContext<'_>| {
            if pts.len() >= 3 {
                ToolOutcome::Committed(ctx.annotations.add(ctx.key, Annotation::Polygon { points: pts }))
            } else {
                ToolOutcome::Changed
            }
        };
        match input.kind {
            InputKind::Press => {
                if pts.len() >= 3 && pts[0].distance(input.mm) <= ctx.tolerance_mm {
                    return close(pts, ctx);
                }
                if pts.last().is_none_or(|p| p.distance(input.mm) > 1e-3) {
                    pts.push(input.mm);
                }
                self.draft = Draft::Points(pts);
                ToolOutcome::Changed
            }
            InputKind::DoubleClick => close(pts, ctx),
            _ => {
                let busy = !pts.is_empty();
                if busy {
                    self.draft = Draft::Points(pts);
                }
                if busy {
                    ToolOutcome::Changed
                } else {
                    ToolOutcome::None
                }
            }
        }
    }

    fn handle_move(&mut self, input: ToolInput, ctx: &mut SliceContext<'_>) -> ToolOutcome {
        match (input.kind, &self.draft) {
            (InputKind::Press, _) => match ctx.annotations.hit_test(ctx.key, input.mm, ctx.tolerance_mm) {
                Some(id) => {
                    self.draft = Draft::Moving(id, input.mm);
                    ToolOutcome::Changed
                }
                None => ToolOutcome::None,
            },
            (InputKind::Drag { .. }, Draft::Moving(id, last)) => {
                let (id, d) = (*id, input.mm - *last);
                if let Some(a) = ctx.annotations.get_mut(id) {
                    a.translate(d);
                }
                self.draft = Draft::Moving(id, input.mm);
                ToolOutcome::Changed
            }
            (InputKind::Release, Draft::Moving(..)) => {
                self.draft = Draft::Idle;
                ToolOutcome::Changed
            }
            _ => ToolOutcome::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrum_domain::Dims3;
    use glam::Vec3;

    struct Fixture {
        volume: Volume,
        view: SliceView,
        window: WindowLevel,
        annotations: AnnotationSet,
    }

    impl Fixture {
        fn new() -> Self {
            let dims = Dims3::new(10, 8, 4);
            let vals: Vec<f32> = (0..dims.voxel_count()).map(|i| i as f32).collect();
            let volume = Volume::from_physical(dims, Vec3::new(2.0, 1.0, 3.0), &vals).unwrap();
            let window = WindowLevel::full_range(volume.range());
            Self { volume, view: SliceView::default(), window, annotations: AnnotationSet::default() }
        }

        fn ctx(&mut self) -> SliceContext<'_> {
            SliceContext {
                key: SliceKey::new(SliceAxis::Axial, 1),
                axis: SliceAxis::Axial,
                index: 1,
                volume: &self.volume,
                view: &mut self.view,
                viewport: Vec2::new(200.0, 160.0),
                window: &mut self.window,
                annotations: &mut self.annotations,
                tolerance_mm: 1.0,
            }
        }
    }

    fn input(kind: InputKind, x: f32, y: f32) -> ToolInput {
        ToolInput { kind, mm: Vec2::new(x, y), screen: Vec2::new(x, y) }
    }

    #[test]
    fn distance_drag_commits_measurement() {
        let mut f = Fixture::new();
        let mut t = ToolController::default();
        t.handle(ToolKind::Distance, input(InputKind::Press, 1.0, 1.0), &mut f.ctx());
        t.handle(ToolKind::Distance, input(InputKind::Drag { delta: Vec2::ONE }, 4.0, 5.0), &mut f.ctx());
        assert_eq!(t.preview(ToolKind::Distance).unwrap().value(), Some(5.0));
        let out = t.handle(ToolKind::Distance, input(InputKind::Release, 4.0, 5.0), &mut f.ctx());
        assert!(matches!(out, ToolOutcome::Committed(_)));
        assert!(!t.is_busy());
        assert_eq!(f.annotations.len(), 1);
    }

    #[test]
    fn tiny_distance_is_discarded() {
        let mut f = Fixture::new();
        let mut t = ToolController::default();
        t.handle(ToolKind::Distance, input(InputKind::Press, 1.0, 1.0), &mut f.ctx());
        t.handle(ToolKind::Distance, input(InputKind::Release, 1.1, 1.0), &mut f.ctx());
        assert!(f.annotations.is_empty());
    }

    #[test]
    fn rect_measures_area() {
        let mut f = Fixture::new();
        let mut t = ToolController::default();
        t.handle(ToolKind::Rect, input(InputKind::Press, 0.0, 0.0), &mut f.ctx());
        t.handle(ToolKind::Rect, input(InputKind::Drag { delta: Vec2::ONE }, 10.0, 5.0), &mut f.ctx());
        let ToolOutcome::Committed(id) = t.handle(ToolKind::Rect, input(InputKind::Release, 10.0, 5.0), &mut f.ctx())
        else {
            panic!("not committed");
        };
        assert_eq!(f.annotations.get(id).unwrap().value(), Some(50.0));
    }

    #[test]
    fn angle_needs_three_clicks() {
        let mut f = Fixture::new();
        let mut t = ToolController::default();
        assert_eq!(t.handle(ToolKind::Angle, input(InputKind::Press, 5.0, 0.0), &mut f.ctx()), ToolOutcome::Changed);
        t.handle(ToolKind::Angle, input(InputKind::Press, 0.0, 0.0), &mut f.ctx());
        t.handle(ToolKind::Angle, input(InputKind::Hover, 0.0, 5.0), &mut f.ctx());
        assert!((t.preview(ToolKind::Angle).unwrap().value().unwrap() - 90.0).abs() < 1e-4);
        let out = t.handle(ToolKind::Angle, input(InputKind::Press, 0.0, 5.0), &mut f.ctx());
        assert!(matches!(out, ToolOutcome::Committed(_)));
        assert!(!t.is_busy());
    }

    #[test]
    fn area_closes_on_first_vertex_or_double_click() {
        let mut f = Fixture::new();
        let mut t = ToolController::default();
        for (x, y) in [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)] {
            t.handle(ToolKind::Area, input(InputKind::Press, x, y), &mut f.ctx());
        }
        let out = t.handle(ToolKind::Area, input(InputKind::Press, 0.2, 0.1), &mut f.ctx());
        let ToolOutcome::Committed(id) = out else { panic!("{out:?}") };
        assert_eq!(f.annotations.get(id).unwrap().value(), Some(50.0));

        for (x, y) in [(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)] {
            t.handle(ToolKind::Area, input(InputKind::Press, x, y), &mut f.ctx());
        }
        let out = t.handle(ToolKind::Area, input(InputKind::DoubleClick, 0.0, 4.0), &mut f.ctx());
        let ToolOutcome::Committed(id) = out else { panic!("{out:?}") };
        assert_eq!(f.annotations.get(id).unwrap().value(), Some(16.0));

        // too few points: nothing committed
        t.handle(ToolKind::Area, input(InputKind::Press, 0.0, 0.0), &mut f.ctx());
        assert_eq!(
            t.handle(ToolKind::Area, input(InputKind::DoubleClick, 0.0, 0.0), &mut f.ctx()),
            ToolOutcome::Changed
        );
        assert_eq!(f.annotations.len(), 2);
    }

    #[test]
    fn move_and_delete() {
        let mut f = Fixture::new();
        let key = SliceKey::new(SliceAxis::Axial, 1);
        let id = f.annotations.add(key, Annotation::Distance { a: Vec2::ZERO, b: Vec2::new(10.0, 0.0) });
        let mut t = ToolController::default();
        t.handle(ToolKind::Move, input(InputKind::Press, 5.0, 0.2), &mut f.ctx());
        t.handle(ToolKind::Move, input(InputKind::Drag { delta: Vec2::ONE }, 5.0, 3.2), &mut f.ctx());
        t.handle(ToolKind::Move, input(InputKind::Release, 5.0, 3.2), &mut f.ctx());
        assert_eq!(f.annotations.get(id).unwrap().points()[0], Vec2::new(0.0, 3.0));
        assert_eq!(t.handle(ToolKind::Delete, input(InputKind::Press, 50.0, 50.0), &mut f.ctx()), ToolOutcome::None);
        assert_eq!(t.handle(ToolKind::Delete, input(InputKind::Press, 5.0, 3.0), &mut f.ctx()), ToolOutcome::Changed);
        assert!(f.annotations.is_empty());
    }

    #[test]
    fn text_is_requested_then_committed() {
        let mut f = Fixture::new();
        let mut t = ToolController::default();
        let out = t.handle(ToolKind::Text, input(InputKind::Press, 3.0, 4.0), &mut f.ctx());
        assert_eq!(out, ToolOutcome::RequestText(Vec2::new(3.0, 4.0)));
        let key = SliceKey::new(SliceAxis::Axial, 1);
        assert!(t.commit_text(&mut f.annotations, key, Vec2::new(3.0, 4.0), "  ").is_none());
        assert!(t.commit_text(&mut f.annotations, key, Vec2::new(3.0, 4.0), "lesion").is_some());
        assert_eq!(f.annotations.len(), 1);
    }

    #[test]
    fn probe_reads_physical_value() {
        let mut f = Fixture::new();
        let mut t = ToolController::default();
        // spacing (2, 1): mm (5, 3) -> pixel (2, 3) on slice 1
        let out = t.handle(ToolKind::Probe, input(InputKind::Hover, 5.0, 3.0), &mut f.ctx());
        let ToolOutcome::Probe(Some(r)) = out else { panic!("{out:?}") };
        assert_eq!(r.voxel, UVec3::new(2, 3, 1));
        assert!((r.value - (2.0 + 3.0 * 10.0 + 80.0)).abs() < 0.01);
        let out = t.handle(ToolKind::Probe, input(InputKind::Hover, -1.0, 3.0), &mut f.ctx());
        assert_eq!(out, ToolOutcome::Probe(None));
    }

    #[test]
    fn pan_zoom_and_window_drag() {
        let mut f = Fixture::new();
        let mut t = ToolController::default();
        t.handle(ToolKind::Pan, input(InputKind::Drag { delta: Vec2::new(3.0, -2.0) }, 0.0, 0.0), &mut f.ctx());
        assert_eq!(f.view.pan, Vec2::new(3.0, -2.0));
        t.handle(ToolKind::Pan, input(InputKind::Scroll { amount: 1.0 }, 100.0, 80.0), &mut f.ctx());
        assert!(f.view.zoom > 1.0);
        t.handle(ToolKind::Pan, input(InputKind::DoubleClick, 0.0, 0.0), &mut f.ctx());
        assert_eq!(f.view, SliceView::default());
        let before = f.window;
        t.handle(ToolKind::WindowLevel, input(InputKind::Drag { delta: Vec2::new(20.0, 0.0) }, 0.0, 0.0), &mut f.ctx());
        assert!(f.window.width > before.width);
    }
}
