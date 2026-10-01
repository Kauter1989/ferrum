//! Colour value types.

/// Linear RGB colour with components in `[0, 1]`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rgb {
    /// Red component.
    pub r: f32,
    /// Green component.
    pub g: f32,
    /// Blue component.
    pub b: f32,
}

impl Rgb {
    /// Pure black.
    pub const BLACK: Rgb = Rgb::new(0.0, 0.0, 0.0);
    /// Pure white.
    pub const WHITE: Rgb = Rgb::new(1.0, 1.0, 1.0);

    /// Creates a colour from float components (not clamped).
    pub const fn new(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b }
    }

    /// Creates a colour from 8-bit components.
    pub fn from_u8(r: u8, g: u8, b: u8) -> Self {
        Self::new(f32::from(r) / 255.0, f32::from(g) / 255.0, f32::from(b) / 255.0)
    }

    /// Linear interpolation `self * (1 - t) + other * t`.
    pub fn lerp(self, other: Rgb, t: f32) -> Rgb {
        Rgb::new(self.r + (other.r - self.r) * t, self.g + (other.g - self.g) * t, self.b + (other.b - self.b) * t)
    }

    /// Returns the colour with every component clamped to `[0, 1]`.
    pub fn clamped(self) -> Rgb {
        Rgb::new(self.r.clamp(0.0, 1.0), self.g.clamp(0.0, 1.0), self.b.clamp(0.0, 1.0))
    }

    /// Converts to an array `[r, g, b]`.
    pub fn to_array(self) -> [f32; 3] {
        [self.r, self.g, self.b]
    }
}

/// 8-bit RGBA colour, the element type of baked lookup tables.
pub type Rgba8 = [u8; 4];

/// Converts a `[0, 1]` float to an 8-bit channel with rounding and clamping.
pub fn unit_to_u8(v: f32) -> u8 {
    // The clamp guarantees the value is inside u8 range before the cast.
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lerp_endpoints() {
        let a = Rgb::new(0.0, 0.5, 1.0);
        let b = Rgb::new(1.0, 0.5, 0.0);
        assert_eq!(a.lerp(b, 0.0), a);
        assert_eq!(a.lerp(b, 1.0), b);
        assert_eq!(a.lerp(b, 0.5), Rgb::new(0.5, 0.5, 0.5));
    }

    #[test]
    fn u8_roundtrip() {
        let c = Rgb::from_u8(255, 128, 0);
        assert_eq!(unit_to_u8(c.r), 255);
        assert_eq!(unit_to_u8(c.g), 128);
        assert_eq!(unit_to_u8(c.b), 0);
        assert_eq!(unit_to_u8(-1.0), 0);
        assert_eq!(unit_to_u8(2.0), 255);
    }
}
