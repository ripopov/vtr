//! Client-side decorations on the existing winit surface. The shadow is a
//! synchronized, input-transparent subsurface below it, not padding inside the
//! app's window. winit's explicit xdg_surface geometry keeps shadows out of
//! snapping, maximize, minimum-size and window-placement calculations.
use super::{Geometry, rounded_region};
use anyhow::Result;
use raw_window_handle::{WaylandDisplayHandle, WaylandWindowHandle};
use std::{fs::File, io::Write, os::fd::AsFd};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{
        wl_buffer, wl_compositor, wl_region, wl_registry, wl_shm, wl_shm_pool, wl_subcompositor,
        wl_subsurface, wl_surface,
    },
};

const SHADOW: i32 = 24;

pub(super) struct Decorations {
    connection: Connection,
    queue: EventQueue<State>,
    state: State,
    compositor: wl_compositor::WlCompositor,
    shm: wl_shm::WlShm,
    parent: wl_surface::WlSurface,
    shadow: wl_surface::WlSurface,
    subsurface: wl_subsurface::WlSubsurface,
}

#[derive(Default)]
struct State {
    buffers: Vec<wl_buffer::WlBuffer>,
}

impl Decorations {
    /// The caller must keep the foreign display and main surface alive.
    pub unsafe fn new(display: WaylandDisplayHandle, surface: WaylandWindowHandle) -> Result<Self> {
        // SAFETY: these live libwayland handles come from winit; the caller
        // retains the native window. This backend borrows, never disconnects it.
        let backend = unsafe {
            wayland_backend::client::Backend::from_foreign_display(display.display.as_ptr().cast())
        };
        let connection = Connection::from_backend(backend);
        let id = unsafe {
            wayland_backend::client::ObjectId::from_ptr(
                wl_surface::WlSurface::interface(),
                surface.surface.as_ptr().cast(),
            )?
        };
        let parent = wl_surface::WlSurface::from_id(&connection, id)?;
        let (globals, queue) = registry_queue_init::<State>(&connection)?;
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=4, ())?;
        let subcompositor: wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
        let shadow = compositor.create_surface(&qh, ());
        let subsurface = subcompositor.get_subsurface(&shadow, &parent, &qh, ());
        subsurface.set_position(-SHADOW, -SHADOW);
        subsurface.place_below(&parent);
        let empty = compositor.create_region(&qh, ());
        shadow.set_input_region(Some(&empty));
        empty.destroy();
        subcompositor.destroy();
        Ok(Self {
            connection,
            queue,
            state: State::default(),
            compositor,
            shm,
            parent,
            shadow,
            subsurface,
        })
    }

    pub fn update(&mut self, geometry: Geometry, changed: bool) -> Result<()> {
        // winit reads the shared display; dispatch only our queue's pending
        // events. No reads, roundtrips or waits in the frame loop.
        self.queue.dispatch_pending(&mut self.state)?;
        if !changed {
            return Ok(());
        }
        let qh = self.queue.handle();
        for (opaque, inset) in [(false, 0), (true, 1)] {
            let region = self.compositor.create_region(&qh, ());
            for [x, y, width, height] in rounded_region(geometry, inset) {
                region.add(x, y, width, height);
            }
            if opaque {
                self.parent.set_opaque_region(Some(&region));
            } else {
                self.parent.set_input_region(Some(&region));
            }
            region.destroy();
        }
        if geometry.radius == 0 {
            self.shadow.attach(None, 0, 0);
        } else {
            let (width, height, pixels) = shadow_pixels(geometry)?;
            let fd =
                rustix::fs::memfd_create(c"atlas-window-shadow", rustix::fs::MemfdFlags::CLOEXEC)?;
            let mut file = File::from(fd);
            file.write_all(&pixels)?;
            let pool = self
                .shm
                .create_pool(file.as_fd(), pixels.len().try_into()?, &qh, ());
            let buffer = pool.create_buffer(
                0,
                width,
                height,
                width * 4,
                wl_shm::Format::Argb8888,
                &qh,
                (),
            );
            pool.destroy();
            self.shadow.attach(Some(&buffer), 0, 0);
            self.shadow.damage(0, 0, width, height);
            self.state.buffers.push(buffer);
        }
        self.shadow.commit();
        // The synchronized child and the new parent regions take effect on
        // eframe's next main-surface commit. Never commit its surface ourselves.
        self.connection.flush()?;
        Ok(())
    }
}

impl Drop for Decorations {
    fn drop(&mut self) {
        self.subsurface.destroy();
        self.shadow.destroy();
        for buffer in self.state.buffers.drain(..) {
            buffer.destroy();
        }
        // parent is borrowed from winit: do not destroy it.
        let _ = self.connection.flush();
    }
}

fn shadow_pixels(g: Geometry) -> Result<(i32, i32, Vec<u8>)> {
    let width = g
        .width
        .checked_add(SHADOW * 2)
        .ok_or_else(|| anyhow::anyhow!("shadow width overflow"))?;
    let height = g
        .height
        .checked_add(SHADOW * 2)
        .ok_or_else(|| anyhow::anyhow!("shadow height overflow"))?;
    let len = i64::from(width)
        .checked_mul(i64::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| anyhow::anyhow!("shadow allocation overflow"))?;
    anyhow::ensure!(
        len > 0 && len <= 256 * 1024 * 1024,
        "window shadow exceeds 256 MiB"
    );
    let mut pixels = vec![0; len as usize];
    // Premultiplied ARGB8888 black shadow. Only evaluate the narrow perimeter;
    // the main window completely covers the transparent interior of this buffer.
    let radius = g.radius as f32;
    let half = [g.width as f32 / 2.0, g.height as f32 / 2.0];
    for y in 0..height {
        let spans = if y > SHADOW + g.radius && y < SHADOW + g.height - g.radius {
            [0..SHADOW + g.radius + 1, SHADOW + g.width - g.radius..width]
        } else {
            [0..width, 0..0]
        };
        for x in spans.into_iter().flatten() {
            let qx = (x as f32 + 0.5 - SHADOW as f32 - half[0]).abs() - half[0] + radius;
            let qy = (y as f32 + 0.5 - SHADOW as f32 - 4.0 - half[1]).abs() - half[1] + radius;
            let distance = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius;
            let alpha =
                (if g.focused { 65.0 } else { 34.0 }) * (-distance.max(0.0).powi(2) / 100.0).exp();
            let pixel = (alpha.round() as u32) << 24;
            let index = ((y * width + x) * 4) as usize;
            pixels[index..index + 4].copy_from_slice(&pixel.to_ne_bytes());
        }
    }
    Ok((width, height, pixels))
}

impl Dispatch<wl_buffer::WlBuffer, ()> for State {
    fn event(
        state: &mut Self,
        buffer: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            buffer.destroy();
            state.buffers.retain(|pending| pending != buffer);
        }
    }
}
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
delegate_noop!(State: ignore wl_compositor::WlCompositor);
delegate_noop!(State: ignore wl_subcompositor::WlSubcompositor);
delegate_noop!(State: ignore wl_subsurface::WlSubsurface);
delegate_noop!(State: ignore wl_surface::WlSurface);
delegate_noop!(State: ignore wl_region::WlRegion);
delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: ignore wl_shm_pool::WlShmPool);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shadow_fades_to_transparent_without_an_opaque_interior() -> Result<()> {
        let geometry = Geometry {
            width: 820,
            height: 560,
            radius: 12,
            focused: true,
        };
        assert!(
            shadow_pixels(Geometry {
                width: i32::MAX,
                ..geometry
            })
            .is_err()
        );
        assert!(
            shadow_pixels(Geometry {
                width: 100_000,
                height: 100_000,
                ..geometry
            })
            .is_err()
        );
        let (width, height, focused) = shadow_pixels(geometry)?;
        let (_, _, inactive) = shadow_pixels(Geometry {
            focused: false,
            ..geometry
        })?;
        let alpha = |pixels: &[u8], x: i32, y: i32| {
            let offset = ((y * width + x) * 4) as usize;
            u32::from_ne_bytes(pixels[offset..offset + 4].try_into().unwrap()) >> 24
        };
        assert_eq!(
            (width, height),
            (geometry.width + SHADOW * 2, geometry.height + SHADOW * 2)
        );
        assert_eq!(alpha(&focused, width / 2, height / 2), 0);
        assert_eq!(alpha(&focused, 0, 0), 0);
        assert_eq!(alpha(&focused, width - 1, height - 1), 0);
        let near = alpha(&focused, SHADOW - 1, height / 2);
        let far = alpha(&focused, 2, height / 2);
        assert!(near > far && near < 255);
        assert!(near > alpha(&inactive, SHADOW - 1, height / 2));
        Ok(())
    }
}
