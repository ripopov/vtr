//! Provision a deterministic multi-page recording for the headless WASM test.
use vtr::*;

fn main() -> anyhow::Result<()> {
    let path = std::env::args_os()
        .nth(1)
        .expect("hierarchy_fixture OUTPUT.vtr");
    let mut writer = Writer::create(path)?;
    let root = writer.add_scope(None, "顶层.λ", ScopeType::Module, "TOP")?;
    let (_, signal) = writer.add_var(
        Some(root),
        "net",
        VarType::Wire,
        Direction::Input,
        SignalKind::Bits {
            width: 1,
            states: 2,
        },
    )?;
    for i in 0..65_537 {
        let scope = writer.add_scope(
            Some(root),
            &format!("cell{i}"),
            ScopeType::Module,
            "NAND2_X1",
        )?;
        writer.add_alias(
            Some(scope),
            "\\pin.λ",
            VarType::Wire,
            Direction::Output,
            signal,
        )?;
    }
    writer.close()?;
    Ok(())
}
