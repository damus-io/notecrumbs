use egui::{lerp, Color32, Pos2, Rgba};

#[derive(Clone, Hash, PartialEq, Eq)]
pub struct Gradient(pub Vec<Color32>);

impl Gradient {
    pub fn linear(left: Color32, right: Color32) -> Self {
        let left = Rgba::from(left);
        let right = Rgba::from(right);

        let n = 255;
        Self(
            (0..=n)
                .map(|i| {
                    let t = i as f32 / n as f32;
                    Color32::from(lerp(left..=right, t))
                })
                .collect(),
        )
    }

    pub fn radial_alpha_gradient(
        center: Pos2,
        radius: f32,
        start_color: Color32,
        end_color: Color32,
    ) -> Self {
        let start_color = Rgba::from(start_color);
        let end_color = Rgba::from(end_color);

        let diameter = (2.0 * radius) as i32;
        let mut pixels = Vec::new();

        for x in 0..diameter {
            for y in 0..diameter {
                let dx = x as f32 - center.x;
                let dy = y as f32 - center.y;
                let distance = (dx * dx + dy * dy).sqrt();

                if distance <= radius {
                    let t = (distance / radius).clamp(0.0, 1.0);
                    let tl = (x as f32) / (diameter as f32);
                    let interpolated_color = Color32::from(lerp(start_color..=end_color, tl));
                    let alpha = (1.0 - t).clamp(0.0, 1.0);

                    pixels.push(Color32::from_rgba_premultiplied(
                        interpolated_color.r(),
                        interpolated_color.g(),
                        interpolated_color.b(),
                        (alpha * 255.0) as u8,
                    ));
                } else {
                    // Handle pixels outside the circle
                    pixels.push(Color32::DEBUG_COLOR);
                }
            }
        }

        Self(pixels)
    }

    /// Do premultiplied alpha-aware blending of the gradient on top of the fill color
    /// in gamma-space.
    pub fn with_bg_fill(self, bg: Color32) -> Self {
        Self(
            self.0
                .into_iter()
                .map(|fg| {
                    let a = fg.a() as f32 / 255.0;
                    Color32::from_rgba_premultiplied(
                        (bg[0] as f32 * (1.0 - a) + fg[0] as f32).round() as u8,
                        (bg[1] as f32 * (1.0 - a) + fg[1] as f32).round() as u8,
                        (bg[2] as f32 * (1.0 - a) + fg[2] as f32).round() as u8,
                        (bg[3] as f32 * (1.0 - a) + fg[3] as f32).round() as u8,
                    )
                })
                .collect(),
        )
    }

    pub fn to_pixel_row(&self) -> Vec<Color32> {
        self.0.clone()
    }
}
