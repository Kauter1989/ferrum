//! The names of the three canvas modes, and what an unknown one does.
//!
//! Pure data, testable on the host: `ferrum-wasm` runs only on wasm32, but the
//! question "does an unknown mode name get refused or silently applied?" is
//! exactly where a wrong answer is invisible — a fallback renders a plausible
//! picture and reports nothing (F1.1-a).

/// Where a frame is drawn: the whole surface, or the MPR layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// One 2D slice, filling the canvas.
    Slice,
    /// A 3D volume render, filling the canvas.
    Volume,
    /// Axial, coronal and sagittal slices beside a 3D render.
    Mpr,
}

impl Mode {
    /// The name the panel uses.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Slice => "slice",
            Mode::Volume => "volume",
            Mode::Mpr => "mpr",
        }
    }

    /// The mode a name means, or an error naming the value.
    ///
    /// Case and surrounding spaces are ignored, so `" MPR "` is the MPR
    /// layout; everything else is refused. There is deliberately no default:
    /// a fallback would show a single slice to a person who asked for MPR, and
    /// nothing in the picture or the log would say so.
    pub fn parse(name: &str) -> Result<Mode, String> {
        let wanted = name.trim();
        for mode in [Mode::Slice, Mode::Volume, Mode::Mpr] {
            if mode.name().eq_ignore_ascii_case(wanted) {
                return Ok(mode);
            }
        }
        Err(format!("unknown mode {name:?}: expected one of slice, volume, mpr"))
    }
}

// The tests live in `tests/mode_names.rs`: this module is compiled there
// directly, so the cases run on the host even though the crate is wasm-only.
