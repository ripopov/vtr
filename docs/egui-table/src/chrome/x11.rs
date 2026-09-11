//! X11 keeps its compositor-managed shadow; only hit testing and opacity hints
//! are changed here. Never shape the visual bounding region: that loses AA.
use super::{Geometry, rounded_region};
use anyhow::Result;
use x11rb::{
    connection::Connection,
    protocol::{
        shape,
        xproto::{self, ConnectionExt as _},
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

pub(super) struct Decorations {
    connection: RustConnection,
    window: u32,
    opaque_region: u32,
}

impl Decorations {
    pub fn new(window: u32) -> Result<Self> {
        let (connection, _) = x11rb::connect(None)?;
        let opaque_region = connection
            .intern_atom(false, b"_NET_WM_OPAQUE_REGION")?
            .reply()?
            .atom;
        Ok(Self {
            connection,
            window,
            opaque_region,
        })
    }

    pub fn update(&self, geometry: Geometry) -> Result<()> {
        let input = rounded_region(geometry, 0)
            .into_iter()
            .map(|[x, y, width, height]| {
                Ok(xproto::Rectangle {
                    x: x.try_into()?,
                    y: y.try_into()?,
                    width: width.try_into()?,
                    height: height.try_into()?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        shape::rectangles(
            &self.connection,
            shape::SO::SET,
            shape::SK::INPUT,
            xproto::ClipOrdering::UNSORTED,
            self.window,
            0,
            0,
            &input,
        )?
        .check()?;
        let opaque: Vec<u32> = rounded_region(geometry, 1)
            .into_iter()
            .flatten()
            .map(|v| v as u32)
            .collect();
        self.connection.change_property32(
            xproto::PropMode::REPLACE,
            self.window,
            self.opaque_region,
            xproto::AtomEnum::CARDINAL,
            &opaque,
        )?;
        self.connection.flush()?;
        Ok(())
    }
}
