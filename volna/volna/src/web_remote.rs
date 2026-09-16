//! Browser transport hosting. Decode, batching and memory admission live in core.
use crate::Workspace;
use gpui_kit::Context;
use std::cell::Cell;
use volna_core::remote::{ClientStep, client::RemoteClient, memory::MemoryBudget, transport};
use volna_core::session::{LoadRequest, LoadResult, OpenSpec};
use wasm_bindgen::{JsCast, JsValue};

thread_local! { static NEXT: Cell<u64> = const { Cell::new(0) }; }

pub(crate) struct Bridge {
    connection: String,
    generation: u64,
    client: RemoteClient,
}

fn call(name: &str, first: &JsValue, second: &JsValue) -> anyhow::Result<()> {
    let global = js_sys::global();
    let callback = js_sys::Reflect::get(&global, &JsValue::from_str(name))
        .map_err(|_| anyhow::anyhow!("missing trace host"))?;
    callback
        .dyn_ref::<js_sys::Function>()
        .ok_or_else(|| anyhow::anyhow!("missing trace host callback {name}"))?
        .call2(&global, first, second)
        .map_err(|_| anyhow::anyhow!("trace host callback failed"))?;
    Ok(())
}
impl Bridge {
    fn send(&self, packet: &transport::Packet) -> anyhow::Result<()> {
        let bytes = transport::encode(packet)?;
        call(
            "volnaTraceSend",
            &self.connection.clone().into(),
            &js_sys::Uint8Array::from(bytes.as_slice()).into(),
        )
    }
}
impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = call(
            "volnaTraceStop",
            &self.connection.clone().into(),
            &JsValue::UNDEFINED,
        );
    }
}

impl Workspace {
    pub(crate) fn sync_remote(&mut self) {
        if self
            .remote
            .as_ref()
            .is_some_and(|r| r.generation != self.app.doc.generation())
        {
            self.remote = None;
        }
        if let Some(bridge) = &mut self.remote {
            bridge.client.sync_demand(&mut self.app);
        }
    }

    pub(crate) fn route_remote(
        &mut self,
        request: LoadRequest,
        cx: &mut Context<Self>,
    ) -> Option<LoadRequest> {
        match request {
            LoadRequest::Open {
                generation,
                spec: OpenSpec::Remote { limits, .. },
            } => {
                self.remote = None;
                let result = (|| {
                    let (memory_bytes, object_bytes) = limits.bytes()?;
                    let connection = NEXT.with(|next| {
                        let id = next
                            .get()
                            .checked_add(1)
                            .expect("connection identity exhausted");
                        next.set(id);
                        id.to_string()
                    });
                    let client = RemoteClient::new(
                        generation,
                        object_bytes,
                        MemoryBudget::new(memory_bytes),
                    )?;
                    self.remote = Some(Bridge {
                        connection: connection.clone(),
                        generation,
                        client,
                    });
                    call("volnaTraceStart", &connection.into(), &JsValue::UNDEFINED)?;
                    self.remote_command()
                })();
                if let Err(error) = result {
                    if self.remote.is_none() {
                        self.app.deliver(LoadResult::Opened {
                            generation,
                            result: Err(error),
                        });
                        self.after(None, cx);
                    } else {
                        self.fail_remote(&format!("{error:#}"), cx);
                    }
                }
                None
            }
            request if request.remote_id().is_some() => {
                let result = match self.remote.as_mut() {
                    Some(bridge) => bridge.client.submit(request),
                    None => Err(request.fail(anyhow::anyhow!("remote connection is closed"))),
                };
                if let Err(result) = result {
                    self.app.deliver(result);
                    cx.notify();
                } else if let Err(error) = self.remote_command() {
                    self.fail_remote(&format!("{error:#}"), cx);
                }
                None
            }
            request => Some(request),
        }
    }

    fn remote_command(&mut self) -> anyhow::Result<()> {
        if let Some(bridge) = &mut self.remote
            && let Some(packet) = bridge.client.take_command()?
        {
            bridge.send(&packet)?;
        }
        Ok(())
    }

    pub(crate) fn remote_frame(
        &mut self,
        connection: &str,
        bytes: Vec<u8>,
        cx: &mut Context<Self>,
    ) {
        let Some(bridge) = self.remote.as_mut().filter(|r| r.connection == connection) else {
            return;
        };
        let result = transport::decode(&bytes).and_then(|packet| bridge.client.accept(packet));
        self.remote_result(result, cx);
    }
    pub(crate) fn remote_continue(&mut self, connection: &str, cx: &mut Context<Self>) {
        let Some(bridge) = self.remote.as_mut().filter(|r| r.connection == connection) else {
            return;
        };
        // A host dispatch can cost a frame. Amortize that delay over a short
        // slice of bounded decoder steps rather than yielding after every
        // small parser checkpoint batch. Keep input/paint opportunities even
        // while a complete transaction track is being indexed.
        let start = volna_core::Instant::now();
        let result = loop {
            let result = bridge.client.step();
            if !matches!(result, Ok(ClientStep::Yield))
                || start.elapsed() >= std::time::Duration::from_millis(2)
            {
                break result;
            }
        };
        self.remote_result(result, cx);
    }
    pub(crate) fn remote_error(&mut self, connection: &str, message: &str, cx: &mut Context<Self>) {
        if self
            .remote
            .as_ref()
            .is_some_and(|r| r.connection == connection)
        {
            self.fail_remote(message, cx);
        }
    }

    fn remote_result(&mut self, result: anyhow::Result<ClientStep>, cx: &mut Context<Self>) {
        let result = result.and_then(|step| {
            let bridge = self
                .remote
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("remote connection is closed"))?;
            match step {
                ClientStep::Yield => call(
                    "volnaTraceYield",
                    &bridge.connection.clone().into(),
                    &JsValue::UNDEFINED,
                ),
                ClientStep::Ack(ack) => bridge.send(&ack),
                ClientStep::Complete { ack, result } => {
                    self.app.deliver(result);
                    bridge.generation = self.app.doc.generation();
                    // End already validated the complete object. Preserve it
                    // for offline use even if sending the final ACK fails.
                    bridge.send(&ack)?;
                    self.after(None, cx);
                    self.remote_command()
                }
            }
        });
        if let Err(error) = result {
            self.fail_remote(&format!("{error:#}"), cx);
        }
    }

    fn fail_remote(&mut self, message: &str, cx: &mut Context<Self>) {
        if let Some(mut bridge) = self.remote.take() {
            for result in bridge.client.disconnect(message) {
                self.app.deliver(result);
            }
        }
        self.after(None, cx);
    }
}
