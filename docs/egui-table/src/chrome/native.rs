//! Demo-only integration with the window system. The main surface stays opaque
//! except at its rounded corners. Input and opaque regions follow that outline.
use std::sync::Arc;

use anyhow::Result;
use eframe::egui;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use winit::window::Window;

#[path = "wayland.rs"]
mod wayland;
#[path = "x11.rs"]
mod x11;

#[derive(Default)]
pub struct WindowFrame {
    // Drop protocol objects before the window (and its borrowed display/surface).
    backend: Option<Backend>,
    window: Option<Arc<Window>>,
    last: Option<Geometry>,
}

enum Backend {
    Wayland(wayland::Decorations),
    X11(x11::Decorations),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Geometry {
    pub width: i32,
    pub height: i32,
    pub radius: i32,
    pub focused: bool,
}

impl WindowFrame {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let Some(window) = cc.winit_window().cloned() else {
            return Self::default();
        };
        let result = (|| -> Result<Option<Backend>> {
            Ok(
                match (
                    window.display_handle()?.as_raw(),
                    window.window_handle()?.as_raw(),
                ) {
                    (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(surface)) => {
                        // SAFETY: eframe owns these handles. Our Arc keeps the window and
                        // display alive until all borrowed Wayland proxies are dropped.
                        Some(Backend::Wayland(unsafe {
                            wayland::Decorations::new(display, surface)?
                        }))
                    }
                    (_, RawWindowHandle::Xlib(handle)) => {
                        Some(Backend::X11(x11::Decorations::new(handle.window as u32)?))
                    }
                    (_, RawWindowHandle::Xcb(handle)) => {
                        Some(Backend::X11(x11::Decorations::new(handle.window.get())?))
                    }
                    _ => None,
                },
            )
        })();
        let backend = match result {
            Ok(backend) => backend,
            Err(error) => {
                eprintln!("Window decorations: {error:#}");
                None
            }
        };
        Self {
            backend,
            window: Some(window),
            last: None,
        }
    }

    pub fn update(&mut self, ctx: &egui::Context) {
        let Some(window) = &self.window else { return };
        let Some(backend) = &mut self.backend else {
            return;
        };
        // Wayland regions use surface coordinates; X11 shapes use physical pixels.
        // egui's zoom factor is independent of the monitor's native scale factor.
        let scale = match backend {
            Backend::Wayland(_) => window.scale_factor(),
            Backend::X11(_) => 1.0,
        };
        let size = window.inner_size();
        let geometry = Geometry {
            width: (size.width as f64 / scale).round() as i32,
            height: (size.height as f64 / scale).round() as i32,
            radius: (super::corner_radius(ctx) as f64 * ctx.pixels_per_point() as f64 / scale)
                .round() as i32,
            focused: ctx.input(|i| i.viewport().focused.unwrap_or(true)),
        };
        if geometry.width <= 0 || geometry.height <= 0 {
            return;
        }
        let changed = self.last != Some(geometry);
        let result = match backend {
            Backend::Wayland(backend) => backend.update(geometry, changed),
            Backend::X11(backend) if changed => backend.update(geometry),
            Backend::X11(_) => Ok(()),
        };
        if let Err(error) = result {
            // Do not keep retrying a failed connection on every repaint.
            eprintln!("Window decorations: {error:#}");
            self.backend = None;
        }
        self.last = Some(geometry);
    }
}

/// Integer scanline rectangles inside a rounded surface. Adjacent identical
/// rows are merged, keeping protocol traffic proportional to the corner radius.
/// `inset` excludes antialiased boundary pixels from the opaque region.
pub(super) fn rounded_region(g: Geometry, inset: i32) -> Vec<[i32; 4]> {
    let radius = g.radius.min(g.width / 2).min(g.height / 2).max(0);
    let mut rows: Vec<[i32; 4]> = Vec::new();
    for y in inset..g.height - inset {
        let dy = if y < radius {
            (radius - y) as f64 - 0.5
        } else if y >= g.height - radius {
            (y - (g.height - radius)) as f64 + 0.5
        } else {
            0.0
        };
        let curve =
            (radius as f64 - ((radius * radius) as f64 - dy * dy).max(0.0).sqrt()).ceil() as i32;
        let x = curve.max(0) + inset;
        let width = g.width - 2 * x;
        if width <= 0 {
            continue;
        }
        if let Some(last) = rows.last_mut()
            && last[0] == x
            && last[2] == width
            && last[1] + last[3] == y
        {
            last[3] += 1;
        } else {
            rows.push([x, y, width, 1]);
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contains(region: &[[i32; 4]], x: i32, y: i32) -> bool {
        region
            .iter()
            .any(|&[rx, ry, w, h]| x >= rx && x < rx + w && y >= ry && y < ry + h)
    }

    #[test]
    fn rounded_regions_exclude_corners_but_keep_resize_edges() {
        for radius in [0, 12, 18, 24] {
            let g = Geometry {
                width: 820,
                height: 560,
                radius,
                focused: true,
            };
            let input = rounded_region(g, 0);
            let opaque = rounded_region(g, 1);
            assert_eq!(contains(&input, 0, 0), radius == 0);
            assert_eq!(contains(&input, 819, 559), radius == 0);
            for (x, y) in [
                (410, 0),
                (410, 559),
                (0, 280),
                (819, 280),
                (7, 7),
                (812, 552),
            ] {
                assert!(
                    contains(&input, x, y),
                    "lost resize target at {x},{y} with radius {radius}"
                );
            }
            for y in 0..g.height {
                for x in 0..g.width {
                    assert_eq!(
                        contains(&input, x, y),
                        contains(&input, g.width - 1 - x, g.height - 1 - y)
                    );
                    if contains(&opaque, x, y) {
                        assert!(contains(&input, x, y));
                        assert!(x > 0 && x < g.width - 1 && y > 0 && y < g.height - 1);
                    }
                }
            }
            assert!(input.len() <= (radius * 2 + 1) as usize);
        }
    }
}
