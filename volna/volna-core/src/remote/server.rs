//! Sequential complete-object service, independent of process and GUI hosting.
use super::history::PackedHistory;
use super::objects::{Metadata, TrackPayload};
use super::transport::{Body, Command, ObjectId, ResponseWriter, read_packet};
use crate::data::SignalRef;
use crate::data::transactions::TrackRef;
use crate::session::Session;
use serde::Serialize;
use std::io::{Read, Write};
use std::sync::Arc;

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
    let session = match open().and_then(|session| {
        check_snapshot()?;
        Ok(session)
    }) {
        Ok(session) => session,
        Err(error) => {
            let mut writer =
                ResponseWriter::new(&mut input, &mut output, session_id, first.request);
            return reply::<_, _, Metadata>(
                &mut writer,
                ObjectId::Metadata,
                Err(error),
                max_object_bytes,
            );
        }
    };
    {
        let mut writer = ResponseWriter::new(&mut input, &mut output, session_id, first.request);
        let metadata = Metadata::from_session(session.as_ref());
        metadata.validate()?;
        reply(
            &mut writer,
            ObjectId::Metadata,
            Ok(metadata),
            max_object_bytes,
        )?;
    }
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
