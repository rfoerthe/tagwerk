//! Zeichnet das App-Symbol (Uhr auf blauem Verlauf) als PNG – wird von build.sh genutzt.

use image::{Rgba, RgbaImage};

fn coverage(d: f32) -> f32 {
    // d < 0 innen; weiche Kante über ~1 Pixel
    (0.5 - d).clamp(0.0, 1.0)
}

fn blend(px: &mut Rgba<u8>, c: [f32; 3], a: f32) {
    if a <= 0.0 {
        return;
    }
    let old_a = px[3] as f32 / 255.0;
    let out_a = a + old_a * (1.0 - a);
    for i in 0..3 {
        let o = px[i] as f32 / 255.0;
        let v = if out_a > 0.0 { (c[i] * a + o * old_a * (1.0 - a)) / out_a } else { 0.0 };
        px[i] = (v * 255.0).round() as u8;
    }
    px[3] = (out_a * 255.0).round() as u8;
}

fn seg_dist(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let (dx, dy) = (bx - ax, by - ay);
    let t = (((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    let (cx, cy) = (ax + t * dx, ay + t * dy);
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

pub fn render(size: u32) -> RgbaImage {
    let s = size as f32;
    let mut img = RgbaImage::from_pixel(size, size, Rgba([0, 0, 0, 0]));
    let margin = s * 0.09;
    let radius = s * 0.2;
    let c = s / 2.0;
    let r_clock = s * 0.30;
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let px = img.get_pixel_mut(x, y);
            // abgerundetes Quadrat
            let qx = (fx - c).abs() - (c - margin - radius);
            let qy = (fy - c).abs() - (c - margin - radius);
            let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - radius;
            let t = ((fy - margin) / (s - 2.0 * margin)).clamp(0.0, 1.0);
            let top = [0.22, 0.62, 0.95];
            let bottom = [0.12, 0.33, 0.78];
            let bg = [
                top[0] + (bottom[0] - top[0]) * t,
                top[1] + (bottom[1] - top[1]) * t,
                top[2] + (bottom[2] - top[2]) * t,
            ];
            blend(px, bg, coverage(outside));
            // Zifferblatt
            let d = ((fx - c).powi(2) + (fy - c).powi(2)).sqrt();
            blend(px, [1.0, 1.0, 1.0], coverage(d - r_clock));
            // Stundenstriche
            for i in 0..12 {
                let ang = i as f32 * std::f32::consts::TAU / 12.0;
                let (sx, sy) = (ang.sin(), -ang.cos());
                let len = if i % 3 == 0 { 0.16 } else { 0.09 };
                let a = (c + sx * r_clock * 0.86, c + sy * r_clock * 0.86);
                let b = (c + sx * r_clock * (0.86 - len), c + sy * r_clock * (0.86 - len));
                let dd = seg_dist(fx, fy, a.0, a.1, b.0, b.1) - s * 0.008;
                blend(px, [0.55, 0.62, 0.72], coverage(dd));
            }
            // Zeiger (10:08)
            let dark = [0.10, 0.16, 0.28];
            let hour = seg_dist(fx, fy, c, c, c - r_clock * 0.40, c - r_clock * 0.22) - s * 0.022;
            blend(px, dark, coverage(hour));
            let minute = seg_dist(fx, fy, c, c, c + r_clock * 0.30, c - r_clock * 0.62) - s * 0.016;
            blend(px, dark, coverage(minute));
            let hub = d - s * 0.03;
            blend(px, [0.93, 0.45, 0.20], coverage(hub));
        }
    }
    img
}
