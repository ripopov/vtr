//! Import foreign trace formats into VTR.

pub mod fst;
pub mod kanata;
pub mod vcd;

fn metadata_attribute(
    writer: &mut vtr::recording::Writer,
    name: &str,
    value: vtr::attributes::Value,
) -> vtr::Result<()> {
    let key = writer.intern(name)?;
    let mut metadata = writer.metadata()?;
    metadata
        .attributes
        .push(vtr::attributes::Attribute { key, value });
    writer.set_metadata(metadata)
}
