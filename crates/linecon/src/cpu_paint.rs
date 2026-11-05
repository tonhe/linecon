//! Paints egui's output with the processor alone. On a machine with no GPU the alternative is
//! Direct3D's software emulation of one, which spent a third of a second of every core on each
//! frame of this window. What egui draws is simple: flat triangles and one font texture.

use std::collections::HashMap;

use egui::epaint::textures::TexturesDelta;
use egui::epaint::{ClippedPrimitive, ImageDelta, Mesh, Primitive, TextureId, Vertex};
use egui::{Color32, ImageData};

struct Texture {
    width: usize,
    height: usize,
    pixels: Vec<Color32>,
}

/// One pixel per `u32`, as 0x00RRGGBB, which is what the window takes directly.
#[derive(Default)]
pub struct Painter {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u32>,
    textures: HashMap<TextureId, Texture>,
}

/// `a * b / 255`, rounded.
#[inline]
fn mul(a: u32, b: u32) -> u32 {
    let x = a * b + 128;
    (x + (x >> 8)) >> 8
}

/// Puts a premultiplied color over what is already there.
#[inline]
fn over(dst: &mut u32, [r, g, b, a]: [u32; 4]) {
    if a == 255 {
        *dst = (r << 16) | (g << 8) | b;
        return;
    }
    let keep = 255 - a;
    let d = *dst;
    *dst = ((r + mul((d >> 16) & 255, keep)).min(255) << 16)
        | ((g + mul((d >> 8) & 255, keep)).min(255) << 8)
        | (b + mul(d & 255, keep)).min(255);
}

impl Painter {
    pub fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        self.pixels.clear();
        self.pixels.resize(width * height, 0);
    }

    fn set_texture(&mut self, id: TextureId, delta: &ImageDelta) {
        let ImageData::Color(image) = &delta.image;
        let [width, height] = image.size;
        match delta.pos {
            None => {
                self.textures.insert(
                    id,
                    Texture {
                        width,
                        height,
                        pixels: image.pixels.clone(),
                    },
                );
            }
            Some([x, y]) => {
                let Some(texture) = self.textures.get_mut(&id) else {
                    return;
                };
                for row in 0..height {
                    let from = row * width;
                    let to = (y + row) * texture.width + x;
                    if to + width <= texture.pixels.len() {
                        texture.pixels[to..to + width]
                            .copy_from_slice(&image.pixels[from..from + width]);
                    }
                }
            }
        }
    }

    pub fn update_textures(&mut self, delta: &TexturesDelta) {
        for (id, images) in &delta.set {
            for image in images {
                self.set_texture(*id, image);
            }
        }
    }

    pub fn free_textures(&mut self, delta: &TexturesDelta) {
        for id in &delta.free {
            self.textures.remove(id);
        }
    }

    pub fn paint(&mut self, background: Color32, primitives: &[ClippedPrimitive], scale: f32) {
        let [r, g, b, _] = background.to_array();
        let fill = (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b);
        self.pixels.fill(fill);
        for primitive in primitives {
            if let Primitive::Mesh(mesh) = &primitive.primitive {
                let clip = [
                    (primitive.clip_rect.min.x * scale).round().max(0.0) as usize,
                    (primitive.clip_rect.min.y * scale).round().max(0.0) as usize,
                    ((primitive.clip_rect.max.x * scale).round().max(0.0) as usize).min(self.width),
                    ((primitive.clip_rect.max.y * scale).round().max(0.0) as usize)
                        .min(self.height),
                ];
                self.paint_mesh(mesh, clip, scale);
            }
        }
    }

    fn paint_mesh(&mut self, mesh: &Mesh, clip: [usize; 4], scale: f32) {
        // Taken out so triangles can write pixels while reading the texture.
        let Some(texture) = self.textures.remove(&mesh.texture_id) else {
            return;
        };
        for &[a, b, c] in mesh.indices.as_chunks::<3>().0 {
            let vertices = [a, b, c].map(|i| &mesh.vertices[i as usize]);
            self.triangle(&texture, vertices, clip, scale);
        }
        self.textures.insert(mesh.texture_id, texture);
    }

    fn triangle(&mut self, texture: &Texture, v: [&Vertex; 3], clip: [usize; 4], scale: f32) {
        let [ax, ay] = [v[0].pos.x * scale, v[0].pos.y * scale];
        let [bx, by] = [v[1].pos.x * scale, v[1].pos.y * scale];
        let [cx, cy] = [v[2].pos.x * scale, v[2].pos.y * scale];
        let area = (bx - ax) * (cy - ay) - (cx - ax) * (by - ay);
        if area.abs() < 1e-3 {
            return;
        }

        // Each value changes at a constant rate across the triangle.
        let gradient = |va: f32, vb: f32, vc: f32| {
            let dx = ((vb - va) * (cy - ay) - (vc - va) * (by - ay)) / area;
            let dy = ((vc - va) * (bx - ax) - (vb - va) * (cx - ax)) / area;
            (dx, dy)
        };
        let channels = |i: usize| [v[0], v[1], v[2]].map(|p| f32::from(p.color.to_array()[i]));
        let color = [channels(0), channels(1), channels(2), channels(3)];
        let color_grad = color.map(|c| gradient(c[0], c[1], c[2]));
        let (u, v_tex) = (
            [v[0].uv.x, v[1].uv.x, v[2].uv.x],
            [v[0].uv.y, v[1].uv.y, v[2].uv.y],
        );
        let u_grad = gradient(u[0], u[1], u[2]);
        let v_grad = gradient(v_tex[0], v_tex[1], v_tex[2]);

        let flat_color = color_grad
            .iter()
            .all(|(dx, dy)| dx.abs() < 1e-4 && dy.abs() < 1e-4);
        let same_texel = {
            let tw = texture.width as f32;
            let th = texture.height as f32;
            (u_grad.0 * tw).abs() < 1e-4
                && (u_grad.1 * tw).abs() < 1e-4
                && (v_grad.0 * th).abs() < 1e-4
                && (v_grad.1 * th).abs() < 1e-4
        };

        let top = ay.min(by).min(cy);
        let bottom = ay.max(by).max(cy);
        let first_row = ((top - 0.5).ceil().max(0.0) as usize).max(clip[1]);
        let last_row = ((bottom - 0.5).ceil().max(0.0) as usize).min(clip[3]);
        let edges = [(ax, ay, bx, by), (bx, by, cx, cy), (cx, cy, ax, ay)];

        for row in first_row..last_row {
            let y = row as f32 + 0.5;
            let mut left = f32::INFINITY;
            let mut right = f32::NEG_INFINITY;
            for (x0, y0, x1, y1) in edges {
                if (y0 <= y) != (y1 <= y) {
                    let x = x0 + (y - y0) / (y1 - y0) * (x1 - x0);
                    left = left.min(x);
                    right = right.max(x);
                }
            }
            if left >= right {
                continue;
            }
            let start = ((left - 0.5).ceil().max(0.0) as usize).max(clip[0]);
            let end = ((right - 0.5).ceil().max(0.0) as usize).min(clip[2]);
            if start >= end {
                continue;
            }
            let span = &mut self.pixels[row * self.width + start..row * self.width + end];
            let (px, py) = (start as f32 + 0.5 - ax, y - ay);

            if flat_color && same_texel {
                let t = sample(
                    texture,
                    u[0] + u_grad.0 * px + u_grad.1 * py,
                    v_tex[0] + v_grad.0 * px + v_grad.1 * py,
                );
                let out = modulate(t, color.map(|c| c[0] as u32));
                if out[3] == 255 {
                    span.fill((out[0] << 16) | (out[1] << 8) | out[2]);
                } else if out != [0; 4] {
                    span.iter_mut().for_each(|dst| over(dst, out));
                }
                continue;
            }

            let mut tu = u[0] + u_grad.0 * px + u_grad.1 * py;
            let mut tv = v_tex[0] + v_grad.0 * px + v_grad.1 * py;
            let mut shade = [0.0f32; 4];
            for (i, value) in shade.iter_mut().enumerate() {
                *value = color[i][0] + color_grad[i].0 * px + color_grad[i].1 * py;
            }
            for dst in span {
                let t = sample(texture, tu, tv);
                let out = modulate(t, shade.map(|c| c.clamp(0.0, 255.0) as u32));
                if out[3] != 0 || out[0] != 0 || out[1] != 0 || out[2] != 0 {
                    over(dst, out);
                }
                tu += u_grad.0;
                tv += v_grad.0;
                for (i, value) in shade.iter_mut().enumerate() {
                    *value += color_grad[i].0;
                }
            }
        }
    }
}

#[inline]
fn sample(texture: &Texture, u: f32, v: f32) -> [u32; 4] {
    let x = ((u * texture.width as f32).max(0.0) as usize).min(texture.width - 1);
    let y = ((v * texture.height as f32).max(0.0) as usize).min(texture.height - 1);
    texture.pixels[y * texture.width + x]
        .to_array()
        .map(u32::from)
}

#[inline]
fn modulate(texel: [u32; 4], color: [u32; 4]) -> [u32; 4] {
    [
        mul(texel[0], color[0]),
        mul(texel[1], color[1]),
        mul(texel[2], color[2]),
        mul(texel[3], color[3]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::epaint::{ClippedPrimitive, Vertex, WHITE_UV};
    use egui::{pos2, Rect};
    use std::sync::Arc;

    fn painter(width: usize, height: usize) -> Painter {
        let mut painter = Painter::default();
        painter.resize(width, height);
        let image = egui::ColorImage::new([2, 2], vec![Color32::WHITE; 4]);
        painter.set_texture(
            TextureId::default(),
            &ImageDelta::full(Arc::new(image), egui::TextureOptions::NEAREST),
        );
        painter
    }

    fn rect_mesh(rect: Rect, color: Color32) -> Mesh {
        let vertex = |x, y| Vertex {
            pos: pos2(x, y),
            uv: WHITE_UV,
            color,
        };
        Mesh {
            indices: vec![0, 1, 2, 0, 2, 3],
            vertices: vec![
                vertex(rect.min.x, rect.min.y),
                vertex(rect.max.x, rect.min.y),
                vertex(rect.max.x, rect.max.y),
                vertex(rect.min.x, rect.max.y),
            ],
            texture_id: TextureId::default(),
        }
    }

    fn draw(p: &mut Painter, mesh: Mesh, clip: Rect, scale: f32) {
        let primitive = ClippedPrimitive {
            clip_rect: clip,
            primitive: Primitive::Mesh(mesh),
        };
        p.paint(Color32::BLACK, &[primitive], scale);
    }

    const WHOLE: Rect = Rect {
        min: pos2(0.0, 0.0),
        max: pos2(100.0, 100.0),
    };

    #[test]
    fn a_rectangle_covers_exactly_its_pixels_with_no_seam() {
        let mut p = painter(6, 6);
        let area = Rect::from_min_max(pos2(1.0, 2.0), pos2(4.0, 5.0));
        draw(
            &mut p,
            rect_mesh(area, Color32::from_rgb(10, 20, 30)),
            WHOLE,
            1.0,
        );
        for y in 0..6 {
            for x in 0..6 {
                let inside = (1..4).contains(&x) && (2..5).contains(&y);
                let want = if inside { 0x000a141e } else { 0 };
                assert_eq!(p.pixels[y * 6 + x], want, "{x},{y}");
            }
        }
    }

    #[test]
    fn nothing_is_drawn_outside_the_clip_or_the_window() {
        let mut p = painter(4, 4);
        let huge = Rect::from_min_max(pos2(-50.0, -50.0), pos2(50.0, 50.0));
        let clip = Rect::from_min_max(pos2(1.0, 1.0), pos2(3.0, 3.0));
        draw(&mut p, rect_mesh(huge, Color32::WHITE), clip, 1.0);
        let lit = p.pixels.iter().filter(|&&px| px != 0).count();
        assert_eq!(lit, 4);
    }

    #[test]
    fn premultiplied_colors_blend_over_what_is_there() {
        let mut dst = 0x00ff_ffff;
        over(&mut dst, [0, 0, 0, 128]);
        let channel = dst & 255;
        assert!((126..=128).contains(&channel), "{channel}");
        let mut dst = 0;
        over(&mut dst, [255, 255, 255, 255]);
        assert_eq!(dst, 0x00ff_ffff);
    }

    #[test]
    fn a_scaled_window_paints_in_physical_pixels() {
        let mut p = painter(8, 8);
        let area = Rect::from_min_max(pos2(0.0, 0.0), pos2(2.0, 2.0));
        draw(&mut p, rect_mesh(area, Color32::WHITE), WHOLE, 2.0);
        assert_eq!(p.pixels.iter().filter(|&&px| px != 0).count(), 16);
    }

    #[test]
    fn a_blended_edge_fades_across_the_triangle() {
        let mut p = painter(11, 1);
        let vertex = |x, y, color| Vertex {
            pos: pos2(x, y),
            uv: WHITE_UV,
            color,
        };
        let mesh = Mesh {
            indices: vec![0, 1, 2, 0, 2, 3],
            vertices: vec![
                vertex(0.0, 0.0, Color32::WHITE),
                vertex(11.0, 0.0, Color32::TRANSPARENT),
                vertex(11.0, 1.0, Color32::TRANSPARENT),
                vertex(0.0, 1.0, Color32::WHITE),
            ],
            texture_id: TextureId::default(),
        };
        draw(&mut p, mesh, WHOLE, 1.0);
        let values: Vec<u32> = p.pixels.iter().map(|px| px & 255).collect();
        assert!(values.windows(2).all(|w| w[0] >= w[1]), "{values:?}");
        assert!(values[0] > 200 && values[10] < 30, "{values:?}");
    }
}
