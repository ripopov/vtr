//! Drive the app without a window and rasterize egui's output in software.
//!
//! Tests feed [`egui::Event`]s, run frames, and read back an RGBA image. The
//! rasterizer draws egui's tessellated triangles with per-vertex colour and
//! texture sampling into a plain buffer: no GPU, no windowing, every platform.

use std::collections::HashMap;

use egui::epaint::{ImageData, Primitive, TextureId};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};

use crate::VolnaApp;

struct Texture {
    width: usize,
    height: usize,
    /// Premultiplied RGBA in `0..=1`.
    pixels: Vec<[f32; 4]>,
}

pub struct Headless {
    pub ctx: egui::Context,
    pub width: usize,
    pub height: usize,
    time: f64,
    pointer: Option<Pos2>,
    textures: HashMap<TextureId, Texture>,
    /// Premultiplied RGBA, row-major.
    frame: Vec<[f32; 4]>,
}

impl Headless {
    pub fn new(width: usize, height: usize) -> Self {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::font_definitions());
        Headless {
            ctx,
            width,
            height,
            time: 0.0,
            pointer: None,
            textures: HashMap::new(),
            frame: vec![[0.0, 0.0, 0.0, 1.0]; width * height],
        }
    }

    fn raw_input(&mut self, events: Vec<Event>) -> RawInput {
        self.time += 1.0 / 60.0;
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                Pos2::ZERO,
                Vec2::new(self.width as f32, self.height as f32),
            )),
            time: Some(self.time),
            predicted_dt: 1.0 / 60.0,
            events,
            focused: true,
            ..Default::default()
        }
    }

    /// Run one frame with `events` and rasterize it.
    pub fn frame(&mut self, app: &mut VolnaApp, events: Vec<Event>) {
        let input = self.raw_input(events);
        let ctx = self.ctx.clone();
        let mut output = ctx.run_ui(input, |ui| app.show(ui));
        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                self.apply_texture(*id, delta);
            }
        }
        for id in &output.textures_delta.free {
            self.textures.remove(id);
        }
        output.textures_delta.clear();
        let primitives = ctx.tessellate(output.shapes, 1.0);
        self.frame.fill([0.0, 0.0, 0.0, 1.0]);
        for clipped in primitives {
            if let Primitive::Mesh(mesh) = clipped.primitive {
                self.draw_mesh(&mesh, clipped.clip_rect);
            }
        }
    }

    /// A few idle frames so animations and deferred layout settle.
    pub fn settle(&mut self, app: &mut VolnaApp, frames: usize) {
        for _ in 0..frames {
            self.frame(app, Vec::new());
        }
    }

    pub fn move_to(&mut self, app: &mut VolnaApp, x: f32, y: f32) {
        self.pointer = Some(Pos2::new(x, y));
        self.frame(app, vec![Event::PointerMoved(Pos2::new(x, y))]);
    }

    pub fn click(&mut self, app: &mut VolnaApp, x: f32, y: f32) {
        self.click_with(app, x, y, PointerButton::Primary, Modifiers::NONE);
    }

    fn click_with(
        &mut self,
        app: &mut VolnaApp,
        x: f32,
        y: f32,
        button: PointerButton,
        modifiers: Modifiers,
    ) {
        let pos = Pos2::new(x, y);
        self.pointer = Some(pos);
        self.frame(app, vec![Event::PointerMoved(pos)]);
        self.frame(
            app,
            vec![Event::PointerButton {
                pos,
                button,
                pressed: true,
                modifiers,
            }],
        );
        self.frame(
            app,
            vec![Event::PointerButton {
                pos,
                button,
                pressed: false,
                modifiers,
            }],
        );
    }

    pub fn drag(&mut self, app: &mut VolnaApp, from: (f32, f32), to: (f32, f32)) {
        let a = Pos2::new(from.0, from.1);
        let b = Pos2::new(to.0, to.1);
        let mid = Pos2::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
        self.frame(app, vec![Event::PointerMoved(a)]);
        self.frame(
            app,
            vec![Event::PointerButton {
                pos: a,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            }],
        );
        self.frame(app, vec![Event::PointerMoved(mid)]);
        self.frame(app, vec![Event::PointerMoved(b)]);
        self.frame(
            app,
            vec![Event::PointerButton {
                pos: b,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            }],
        );
        self.pointer = Some(b);
    }

    pub fn key(&mut self, app: &mut VolnaApp, key: Key, modifiers: Modifiers) {
        self.frame(
            app,
            vec![Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
        );
        self.frame(
            app,
            vec![Event::Key {
                key,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers,
            }],
        );
    }

    pub fn text(&mut self, app: &mut VolnaApp, text: &str) {
        self.frame(app, vec![Event::Text(text.to_owned())]);
    }

    pub fn zoom(&mut self, app: &mut VolnaApp, factor: f32) {
        let mut events = Vec::new();
        if let Some(p) = self.pointer {
            events.push(Event::PointerMoved(p));
        }
        events.push(Event::Zoom(factor));
        self.frame(app, events);
    }

    /// The last frame as straight-alpha RGBA bytes with alpha forced opaque.
    pub fn rgba(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.width * self.height * 4);
        for p in &self.frame {
            out.push((p[0].clamp(0.0, 1.0) * 255.0).round() as u8);
            out.push((p[1].clamp(0.0, 1.0) * 255.0).round() as u8);
            out.push((p[2].clamp(0.0, 1.0) * 255.0).round() as u8);
            out.push(255);
        }
        out
    }

    /// True when the frame has more than one distinct colour.
    pub fn is_nonblank(&self) -> bool {
        let first = self.frame[0];
        self.frame.iter().any(|p| *p != first)
    }

    fn apply_texture(&mut self, id: TextureId, delta: &egui::epaint::ImageDelta) {
        let ImageData::Color(image) = &delta.image;
        let [w, h] = image.size;
        let to_f = |c: egui::Color32| {
            let [r, g, b, a] = c.to_array();
            [
                r as f32 / 255.0,
                g as f32 / 255.0,
                b as f32 / 255.0,
                a as f32 / 255.0,
            ]
        };
        match delta.pos {
            None => {
                self.textures.insert(
                    id,
                    Texture {
                        width: w,
                        height: h,
                        pixels: image.pixels.iter().map(|c| to_f(*c)).collect(),
                    },
                );
            }
            Some([x0, y0]) => {
                if let Some(t) = self.textures.get_mut(&id) {
                    for y in 0..h {
                        for x in 0..w {
                            let (tx, ty) = (x0 + x, y0 + y);
                            if tx < t.width && ty < t.height {
                                t.pixels[ty * t.width + tx] = to_f(image.pixels[y * w + x]);
                            }
                        }
                    }
                }
            }
        }
    }

    fn sample(tex: &Texture, u: f32, v: f32) -> [f32; 4] {
        // Bilinear, clamped.
        let fx = (u * tex.width as f32 - 0.5).clamp(0.0, (tex.width - 1) as f32);
        let fy = (v * tex.height as f32 - 0.5).clamp(0.0, (tex.height - 1) as f32);
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(tex.width - 1), (y0 + 1).min(tex.height - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let px = |x: usize, y: usize| tex.pixels[y * tex.width + x];
        let mut out = [0.0; 4];
        for (i, o) in out.iter_mut().enumerate() {
            let top = px(x0, y0)[i] * (1.0 - tx) + px(x1, y0)[i] * tx;
            let bot = px(x0, y1)[i] * (1.0 - tx) + px(x1, y1)[i] * tx;
            *o = top * (1.0 - ty) + bot * ty;
        }
        out
    }

    fn draw_mesh(&mut self, mesh: &egui::epaint::Mesh, clip: Rect) {
        let tex = self.textures.get(&mesh.texture_id);
        let x_min = clip.min.x.max(0.0).floor() as i64;
        let y_min = clip.min.y.max(0.0).floor() as i64;
        let x_max = clip.max.x.min(self.width as f32).ceil() as i64;
        let y_max = clip.max.y.min(self.height as f32).ceil() as i64;
        for tri in mesh.indices.chunks(3) {
            let [a, b, c] = [
                &mesh.vertices[tri[0] as usize],
                &mesh.vertices[tri[1] as usize],
                &mesh.vertices[tri[2] as usize],
            ];
            let area = edge(a.pos, b.pos, c.pos);
            if area.abs() < 1e-12 {
                continue;
            }
            let bx0 = (a.pos.x.min(b.pos.x).min(c.pos.x).floor() as i64).max(x_min);
            let by0 = (a.pos.y.min(b.pos.y).min(c.pos.y).floor() as i64).max(y_min);
            let bx1 = (a.pos.x.max(b.pos.x).max(c.pos.x).ceil() as i64).min(x_max);
            let by1 = (a.pos.y.max(b.pos.y).max(c.pos.y).ceil() as i64).min(y_max);
            let colors = [a, b, c].map(|v| {
                let [r, g, bb, al] = v.color.to_array();
                [
                    r as f32 / 255.0,
                    g as f32 / 255.0,
                    bb as f32 / 255.0,
                    al as f32 / 255.0,
                ]
            });
            for py in by0..by1 {
                for px in bx0..bx1 {
                    let p = Pos2::new(px as f32 + 0.5, py as f32 + 0.5);
                    let w0 = edge(b.pos, c.pos, p) / area;
                    let w1 = edge(c.pos, a.pos, p) / area;
                    let w2 = edge(a.pos, b.pos, p) / area;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let mut src = [0.0f32; 4];
                    for i in 0..4 {
                        src[i] = colors[0][i] * w0 + colors[1][i] * w1 + colors[2][i] * w2;
                    }
                    if let Some(tex) = tex {
                        let u = a.uv.x * w0 + b.uv.x * w1 + c.uv.x * w2;
                        let v = a.uv.y * w0 + b.uv.y * w1 + c.uv.y * w2;
                        let t = Self::sample(tex, u, v);
                        for i in 0..4 {
                            src[i] *= t[i];
                        }
                    }
                    let dst = &mut self.frame[py as usize * self.width + px as usize];
                    for i in 0..4 {
                        dst[i] = src[i] + dst[i] * (1.0 - src[3]);
                    }
                }
            }
        }
    }
}

fn edge(a: Pos2, b: Pos2, p: Pos2) -> f32 {
    (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)
}
