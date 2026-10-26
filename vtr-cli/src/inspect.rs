use anyhow::{Context, Result, bail};
use std::{collections::BTreeMap, path::Path};
use vtr::{
    Direction, NodeId, ScopeType, VarType,
    declarations::NodeData,
    records::{RecordKind, RecordQuery},
    trace::{Reader, Record, Severity},
    transactions::TransactionQuery,
};

pub fn info(path: &Path) -> Result<()> {
    let reader = Reader::open(path)?;
    let metadata = reader.metadata();
    println!("file:        {}", path.display());
    println!(
        "version:     {}.{}",
        vtr::framing::VERSION.0,
        vtr::framing::VERSION.1
    );
    println!("recording:   {}", reader.identity());
    println!(
        "completion:  {:?} ({} bytes dropped)",
        reader.completion(),
        reader.dropped_bytes()
    );
    println!("writer:      {}", metadata.writer);
    println!("date:        {}", metadata.date);
    println!("file type:   {}", metadata.file_type);
    println!("timescale:   1e{} s", metadata.timescale);
    println!("time zero:   {}", metadata.time_zero);
    if let Some(window) = reader.waveform_time_range() {
        println!("wave range:  {} .. {}", window.first, window.last);
    }
    if !metadata.comment.is_empty() {
        println!("comment:     {}", metadata.comment);
    }
    for attribute in &metadata.attributes {
        println!(
            "attr:        {} = {}",
            reader.string(attribute.key)?,
            reader.format_value(&attribute.value)?
        );
    }
    let (mut scopes, mut variables, mut generators) = (0, 0, 0);
    for node in &reader.declarations().nodes {
        match node.data {
            NodeData::Scope { .. } => scopes += 1,
            NodeData::Signal { .. } => variables += 1,
            NodeData::Generator { .. } => generators += 1,
            NodeData::EnumTable { .. } | NodeData::Callsite { .. } => (),
        }
    }
    println!(
        "hierarchy:   {scopes} scopes, {variables} variables ({} signals), {generators} generators",
        reader.signal_count()
    );
    println!("strings:     {}", reader.declarations().strings.len());
    println!("datatypes:   {}", reader.types().len());
    println!("wave blocks: {}", reader.block_count());
    let report = reader.validate()?;
    println!(
        "records:     {} ({} relations, {} unresolved references)",
        report.records,
        report.relations,
        report.missing.len()
    );
    println!("blackout:    {} transitions", reader.blackouts().len());
    let mut sections = BTreeMap::<u32, (usize, u64)>::new();
    for section in reader.sections() {
        let entry = sections.entry(section.kind).or_default();
        entry.0 += 1;
        entry.1 += section.length + 24;
    }
    for (kind, (count, bytes)) in sections {
        let name = match kind {
            1 => "Metadata",
            2 => "Strings",
            3 => "Hierarchy",
            4 => "Waveforms",
            5 => "Transactions",
            6 => "Blackout",
            9 => "Records",
            10 => "Datatypes",
            _ => "Optional",
        };
        println!("section {name:<12} x{count:<6} {bytes} bytes");
    }
    Ok(())
}

pub fn hierarchy(path: &Path, max_depth: usize, variables: bool, sizes: bool) -> Result<()> {
    let reader = Reader::open(path)?;
    if sizes {
        let (nodes, sizes) = reader.scope_sizes();
        let mut depths = vec![0; nodes.len()];
        println!("scope  scopes  variables  signals");
        for (index, &node) in nodes.iter().enumerate() {
            depths[index] = sizes
                .parent(index as u32)
                .map_or(0, |parent| depths[parent as usize] + 1);
            if depths[index] < max_depth {
                println!(
                    "{}{}  {}  {}  {}",
                    "  ".repeat(depths[index]),
                    reader.name(node)?,
                    sizes.scopes(index as u32),
                    sizes.variables(index as u32),
                    sizes.signals(index as u32)
                );
            }
        }
        return Ok(());
    }
    let mut pending: Vec<_> = reader
        .children(None)
        .iter()
        .rev()
        .map(|&node| (node, 0))
        .collect();
    while let Some((id, depth)) = pending.pop() {
        if depth >= max_depth {
            continue;
        }
        let node = reader.node(id)?;
        let name = reader.name(id)?;
        let indent = "  ".repeat(depth);
        match &node.data {
            NodeData::Scope {
                scope_type,
                component,
            } => println!(
                "{indent}{name} [{} {}]",
                ScopeType::from_code(*scope_type).name(),
                reader.string(*component)?
            ),
            NodeData::Signal {
                signal,
                var_type,
                direction,
                alias,
            } => {
                if !variables {
                    continue;
                }
                println!(
                    "{indent}{name} : {} {} {:?} sig#{}{}",
                    VarType::from_code(*var_type).name(),
                    Direction::from_u8(*direction).name(),
                    reader.signal_kind(*signal)?,
                    signal.0,
                    if *alias { " (alias)" } else { "" }
                );
            }
            NodeData::Generator { kind, payload_type } => println!(
                "{indent}{name} [generator {} type#{payload_type}]",
                reader.string(*kind)?
            ),
            NodeData::Callsite { payload_type } => {
                println!("{indent}{name} [callsite type#{payload_type}]");
            }
            NodeData::EnumTable { entries } => {
                println!("{indent}{name} [enum {} literals]", entries.len());
            }
        }
        for attribute in &node.attributes {
            println!(
                "{indent}  @{} = {}",
                reader.string(attribute.key)?,
                reader.format_value(&attribute.value)?
            );
        }
        pending.extend(
            reader
                .children(Some(id))
                .iter()
                .rev()
                .map(|&node| (node, depth + 1)),
        );
    }
    Ok(())
}

fn show(reader: &Reader, record: &Record, indent: &str) -> Result<()> {
    let source = record
        .generator()
        .map(|generator| reader.full_path(NodeId(generator)))
        .transpose()?
        .unwrap_or_default();
    println!(
        "{indent}record {} {source} [{} .. {}] status={} kind={:?}{}",
        record.id(),
        record.begin(),
        record.end(),
        record.status(),
        record.kind(),
        record
            .parent()
            .map(|parent| format!(" parent={parent}"))
            .unwrap_or_default()
    );
    println!(
        "{indent}    payload = {}",
        reader.format_payload(&record.payload())?
    );
    for attribute in record.attributes() {
        println!(
            "{indent}    {} = {}",
            reader.string(attribute.key)?,
            reader.format_value(&attribute.value)?
        );
    }
    for event in record.events() {
        println!(
            "{indent}    event {} @{} {}",
            event.id,
            event.time,
            reader.string(event.name)?
        );
    }
    for stage in record.stages() {
        println!(
            "{indent}    stage {} {} lane={} [{} .. {}]",
            stage.id,
            reader.string(stage.name)?,
            reader.string(stage.lane)?,
            stage.begin,
            stage
                .end
                .map_or_else(|| "open".into(), |end| end.to_string())
        );
    }
    Ok(())
}

pub fn transactions(
    path: &Path,
    scope: Option<&str>,
    window: (u64, u64),
    max: usize,
    ids: &[u64],
) -> Result<()> {
    let reader = Reader::open(path)?;
    if !ids.is_empty() {
        for &id in ids {
            let record = reader
                .lookup(id)?
                .with_context(|| format!("record {id} not found"))?;
            show(&reader, &record, "")?;
            if matches!(record.kind(), RecordKind::Transaction | RecordKind::Clock) {
                let expansion = reader.expand(id)?;
                for located in expansion.located {
                    show(&reader, &located, "    ")?;
                }
                if !expansion.missing_blocks.is_empty() {
                    println!("    missing child blocks: {:?}", expansion.missing_blocks);
                }
            }
            for incoming in [false, true] {
                let mut relations = Vec::new();
                reader.relations(id, incoming, |relation| {
                    relations.push(relation);
                    true
                })?;
                for relation in relations {
                    println!(
                        "    {} {} {}",
                        if incoming { "<-" } else { "->" },
                        reader.string(relation.kind)?,
                        if incoming {
                            relation.source
                        } else {
                            relation.destination
                        }
                    );
                }
            }
        }
        return Ok(());
    }
    let scope = find_scope(&reader, scope)?;
    let mut count = 0usize;
    let mut records = Vec::new();
    reader.transactions(
        TransactionQuery {
            begin: window.0,
            end: window.1,
            ..Default::default()
        },
        |record| {
            if record
                .generator()
                .is_some_and(|generator| reader.is_within(NodeId(generator), scope))
            {
                count += 1;
                if records.len() < max {
                    records.push(record);
                }
            }
            true
        },
    )?;
    for record in records {
        show(&reader, &record, "")?;
    }
    if count > max {
        println!("... {} more", count - max);
    }
    Ok(())
}

pub fn logs(
    path: &Path,
    scope: Option<&str>,
    window: (u64, u64),
    max: usize,
    severity: Severity,
    sites: bool,
) -> Result<()> {
    let reader = Reader::open(path)?;
    let scope = find_scope(&reader, scope)?;
    let sources: BTreeMap<_, _> = reader
        .log_callsites()?
        .into_iter()
        .filter(|source| source.severity >= severity)
        .map(|source| (source.callsite, source))
        .collect();
    if sites {
        for source in sources.values() {
            println!(
                "site {} {} {} type#{}",
                source.callsite,
                reader.full_path(NodeId(source.callsite))?,
                source.severity.name(),
                source.payload_type
            );
        }
        return Ok(());
    }
    let mut shown = 0usize;
    let mut error = None;
    let loggers: Vec<_> = reader
        .declarations()
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(id, node)| {
            if let NodeData::Generator { kind, .. } = node.data {
                (reader.string(kind).ok() == Some("LOG")
                    && reader.is_within(NodeId(id as u32), scope))
                .then_some(id as u32)
            } else {
                None
            }
        })
        .collect();
    'streams: for generator in loggers {
        for source in sources.values() {
            reader.records(
                RecordQuery {
                    kind: Some(RecordKind::Log),
                    generator: Some(generator),
                    callsite: Some(source.callsite),
                    begin: window.0,
                    end: window.1,
                    ..Default::default()
                },
                |record| {
                    if shown >= max {
                        return false;
                    }
                    match reader.format_log_bytes(&record) {
                        Ok(bytes) => {
                            let message = String::from_utf8(bytes).unwrap_or_else(|error| {
                                use std::fmt::Write;
                                let mut text = String::from("[non-UTF-8 bytes] ");
                                for byte in error.into_bytes() {
                                    write!(text, "{byte:02x}").unwrap();
                                }
                                text
                            });
                            println!(
                                "{} {:<5} {}: {}",
                                record.begin(),
                                source.severity.name(),
                                reader
                                    .full_path(NodeId(record.generator().unwrap()))
                                    .unwrap(),
                                message
                            );
                        }
                        Err(failure) => {
                            error = Some(failure);
                            return false;
                        }
                    }
                    shown += 1;
                    true
                },
            )?;
            if shown >= max || error.is_some() {
                break 'streams;
            }
        }
    }
    if let Some(error) = error {
        return Err(error.into());
    }
    Ok(())
}

pub fn clocks(path: &Path) -> Result<()> {
    let reader = Reader::open(path)?;
    for (id, node) in reader.declarations().nodes.iter().enumerate() {
        if !matches!(node.data, NodeData::Generator { .. })
            || reader.declarations().generators.get(id as u32)?.kind != RecordKind::Clock
        {
            continue;
        }
        let mut stretches = Vec::new();
        reader.transactions(
            TransactionQuery {
                generator: Some(id as u32),
                ..Default::default()
            },
            |record| {
                stretches.push(record);
                true
            },
        )?;
        stretches.sort_by_key(Record::begin);
        println!(
            "clock {id} {}: {} stretches",
            reader.full_path(NodeId(id as u32))?,
            stretches.len()
        );
        for stretch in stretches {
            println!(
                "    [{} .. {}] cycles {} .. {}{}",
                stretch.begin(),
                stretch.end(),
                reader.cycle_at(id as u32, stretch.begin())?.unwrap(),
                reader.cycle_at(id as u32, stretch.end())?.unwrap(),
                if stretch.status() == vtr::Status::Open {
                    ", running at close"
                } else {
                    ""
                }
            );
        }
    }
    Ok(())
}
fn find_scope(reader: &Reader, name: Option<&str>) -> Result<Option<NodeId>> {
    name.map(|name| {
        let id = reader
            .find(name)
            .with_context(|| format!("scope {name:?} not found"))?;
        if !matches!(reader.node(id)?.data, NodeData::Scope { .. }) {
            bail!("{name:?} is not a scope");
        }
        Ok(id)
    })
    .transpose()
}
