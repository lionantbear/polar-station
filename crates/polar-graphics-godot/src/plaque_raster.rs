//! The existing laminate pixel recipe, baked on the CPU without per-pixel
//! GDScript/engine crossings. Image/texture ownership remains with Godot.
use godot::classes::RefCounted;
use godot::prelude::*;

#[derive(GodotClass)]
#[class(base=RefCounted, init)]
struct PolarPlaqueRaster {
    base: Base<RefCounted>,
}

#[godot_api]
impl PolarPlaqueRaster {
    #[func]
    fn glass_rgba8(
        &self,
        contour: PackedVector2Array,
        half_size: Vector2,
        unit: f64,
    ) -> PackedByteArray {
        let points: Vec<_> = contour.as_slice().iter().map(|p| [p.x, p.y]).collect();
        PackedByteArray::from(raster(&points, [half_size.x, half_size.y], unit).as_slice())
    }
}

fn smoothstep(low: f64, high: f64, value: f64) -> f64 {
    let t = ((value - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn rgb(hex: u32) -> [f32; 3] {
    [
        ((hex >> 16) & 255) as f32 / 255.0,
        ((hex >> 8) & 255) as f32 / 255.0,
        (hex & 255) as f32 / 255.0,
    ]
}

fn mix(a: [f32; 3], b: [f32; 3], weight: f64) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * weight as f32)
}

fn raster(contour: &[[f32; 2]], half: [f32; 2], unit: f64) -> Vec<u8> {
    if contour.len() < 3
        || !unit.is_finite()
        || unit <= 0.0
        || half.iter().any(|x| !x.is_finite() || *x <= 0.0)
        || contour.iter().flatten().any(|x| !x.is_finite())
    {
        return Vec::new();
    }
    let mut bytes = Vec::with_capacity(512 * 128 * 4);
    for y in 0..128 {
        let uv_y = y as f32 / 127.0;
        for x in 0..512 {
            let uv_x = x as f32 / 511.0;
            let point = [
                ((uv_x as f64 - 0.5) * 2.0 * half[0] as f64) as f32,
                ((0.5 - uv_y as f64) * 2.0 * half[1] as f64) as f32,
            ];
            let mut distance = f32::INFINITY;
            for i in 0..contour.len() {
                let a = contour[i];
                let b = contour[(i + 1) % contour.len()];
                let edge = [b[0] - a[0], b[1] - a[1]];
                let offset = [point[0] - a[0], point[1] - a[1]];
                let length_squared = edge[0] * edge[0] + edge[1] * edge[1];
                let dot = offset[0] * edge[0] + offset[1] * edge[1];
                let t = if length_squared == 0.0 {
                    0.0
                } else {
                    (dot as f64 / length_squared as f64).clamp(0.0, 1.0) as f32
                };
                let dx = point[0] - (a[0] + edge[0] * t);
                let dy = point[1] - (a[1] + edge[1] * t);
                distance = distance.min((dx * dx + dy * dy).sqrt());
            }
            let mut tone = mix(rgb(0x263945), rgb(0x101c26), smoothstep(0.0, 0.85, uv_y as f64));
            let gasket = 1.0 - smoothstep(0.007 * unit, 0.027 * unit, distance as f64);
            let seal = 1.0
                - smoothstep(
                    0.005 * unit,
                    0.014 * unit,
                    (distance as f64 - 0.043 * unit).abs(),
                );
            tone = mix(tone, rgb(0x080f17), gasket * 0.65);
            tone = mix(tone, rgb(0x78939c), seal * (0.26 + (0.08 - 0.26) * uv_y as f64));
            bytes.extend(tone.map(|c| (c * 255.0).clamp(0.0, 255.0) as u8));
            bytes.push(255);
        }
    }
    bytes
}


