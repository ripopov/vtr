//! Sequential complete-object service, independent of process and GUI hosting.
use serde::Serialize;
use std::io::{Read, Write};
use std::sync::Arc;
use volna_trace::data::SignalRef;
use volna_trace::data::transactions::TrackRef;
use volna_trace::remote::hierarchy::Header;
use volna_trace::remote::history::PackedHistory;
use volna_trace::remote::objects::TrackPayload;
use volna_trace::remote::transport::{Body, Command, ObjectId, ResponseWriter, read_packet};
use volna_trace::session::Session;

fn reply<R: Read, W: Write, T: Serialize>(
    writer: &mut ResponseWriter<R, W>,
    object: ObjectId,
    result: anyhow::Result<T>,
    limit: u64,
) -> anyhow::Result<()> {
    let result = result.and_then(|value| writer.object(object, &value, limit));
    if let Err(error) = result {
        if writer.is_failed() {
            return Err(error);
        }
        writer.send(Body::Error {
            object,
            message: format!("{error:#}").chars().take(1024).collect(),
        })?;
    }
    Ok(())
}

fn open_object<R: Read, W: Write, T: Serialize>(
    writer: &mut ResponseWriter<R, W>,
    object: ObjectId,
    value: &T,
    limit: u64,
) -> anyhow::Result<bool> {
    match writer.object(object, value, limit) {
        Ok(()) => Ok(true),
        Err(error) if writer.is_failed() => Err(error),
        Err(error) => {
            writer.send(Body::Error {
                object,
                message: format!("{error:#}").chars().take(1024).collect(),
            })?;
            Ok(false)
        }
    }
}

/// Serve one recording. The host assigns a fresh nonzero session identity and
/// supplies the immutable-file check. EOF or Close drops all service-owned data.
pub fn serve(
    mut input: impl Read,
    mut output: impl Write,
    session_id: u64,
    open: impl FnOnce() -> anyhow::Result<Arc<dyn Session>>,
    mut check_snapshot: impl FnMut() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    anyhow::ensure!(session_id != 0, "invalid server session identity");
    let first =
        read_packet(&mut input)?.ok_or_else(|| anyhow::anyhow!("connection closed before Open"))?;
    anyhow::ensure!(
        first.session == 0 && first.sequence == 0 && first.request != 0,
        "invalid Open envelope"
    );
    let Body::Command(Command::Open { max_object_bytes }) = first.body else {
        anyhow::bail!("first command must be Open");
    };
    let session = {
        let mut writer = ResponseWriter::new(&mut input, &mut output, session_id, first.request);
        let session = match open().and_then(|session| {
            check_snapshot()?;
            Ok(session)
        }) {
            Ok(session) => session,
            Err(error) => {
                return reply::<_, _, Header>(
                    &mut writer,
                    ObjectId::Metadata,
                    Err(error),
                    max_object_bytes,
                );
            }
        };
        let header = Header::borrowed(
            session.as_ref(),
            gethostname::gethostname().to_string_lossy().into_owned(),
        )?;
        let sizes = session
            .scope_sizes()
            .unwrap_or_else(|| Arc::new(volna_trace::data::ScopeSizes::count(session.hierarchy())));
        if !open_object(&mut writer, ObjectId::Metadata, &header, max_object_bytes)? {
            return Ok(());
        }
        // The client waits for exactly the pages the header's counts imply;
        // sending fewer would leave both sides waiting.
        let hierarchy = session.hierarchy();
        let pages = |n: usize| n.div_ceil(volna_trace::remote::hierarchy::PAGE_ENTRIES);
        let mut sent = 0;
        for (page, buffer) in
            volna_trace::remote::hierarchy::scope_pages(hierarchy, &sizes).enumerate()
        {
            sent += 1;
            if !open_object(
                &mut writer,
                ObjectId::Scopes(page as u32),
                &buffer,
                max_object_bytes,
            )? {
                return Ok(());
            }
        }
        anyhow::ensure!(
            sent == pages(hierarchy.scope_count()),
            "scope pages do not match the header"
        );
        let mut sent = 0;
        for (page, buffer) in volna_trace::remote::hierarchy::var_pages(hierarchy).enumerate() {
            sent += 1;
            if !open_object(
                &mut writer,
                ObjectId::Variables(page as u32),
                &buffer,
                max_object_bytes,
            )? {
                return Ok(());
            }
        }
        anyhow::ensure!(
            sent == pages(hierarchy.var_count()),
            "variable pages do not match the header"
        );
        session
    };
    let mut last_request = first.request;
    while let Some(packet) = read_packet(&mut input)? {
        anyhow::ensure!(
            packet.session == session_id && packet.sequence == 0 && packet.request > last_request,
            "invalid request identity"
        );
        last_request = packet.request;
        let Body::Command(command) = packet.body else {
            anyhow::bail!("expected command");
        };
        if command == Command::Close {
            return Ok(());
        }
        check_snapshot()?;
        let mut writer = ResponseWriter::new(&mut input, &mut output, session_id, packet.request);
        match command {
            Command::Signals(ids) => {
                let mut unique = Vec::with_capacity(ids.len());
                for id in ids {
                    if !unique.contains(&SignalRef(id)) {
                        unique.push(SignalRef(id));
                    }
                }
                let results = session.load_signals(&unique);
                check_snapshot()?;
                for (id, result) in results {
                    reply(
                        &mut writer,
                        ObjectId::Signal(id.0),
                        result.and_then(|history| {
                            PackedHistory::from_history_with_limit(
                                history.as_ref(),
                                max_object_bytes,
                            )
                        }),
                        max_object_bytes,
                    )?;
                }
            }
            Command::Activity { build } => {
                let cache = vtr::activity::default_cache_dir();
                let result = (|| {
                    if build && session.activity().is_none() {
                        // This server owns the scan. A disconnected client's
                        // scan may finish and publish for subsequent clients.
                        let options = vtr::activity::BuildOptions {
                            control: Some(Arc::new(vtr::activity::BuildControl::default())),
                            ..Default::default()
                        };
                        let budget = volna_trace::remote::memory::MemoryBudget::new(2 << 30);
                        session.build_activity(&options, &budget, cache.as_deref())?;
                    }
                    session.activity_image(cache.as_deref())
                })();
                check_snapshot()?;
                #[derive(Serialize)]
                struct Image<'a>(#[serde(serialize_with = "serialize_bytes")] &'a [u8]);
                reply(
                    &mut writer,
                    ObjectId::Activity,
                    result
                        .as_ref()
                        .map(|bytes| Image(bytes))
                        .map_err(|error| anyhow::anyhow!("{error:#}")),
                    max_object_bytes,
                )?;
            }
            Command::Track(id) => {
                let result = session.load_track(TrackRef(id));
                check_snapshot()?;
                reply(
                    &mut writer,
                    ObjectId::Track(id),
                    result
                        .as_ref()
                        .map(TrackPayload::from_loaded)
                        .map_err(|error| anyhow::anyhow!("{error:#}")),
                    max_object_bytes,
                )?;
            }
            _ => anyhow::bail!("unexpected command after Open"),
        }
    }
    Ok(())
}

fn serialize_bytes<S: serde::Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_bytes(bytes)
}
