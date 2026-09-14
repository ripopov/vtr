//! WASM host for the VS Code opaque-byte courier.
use crate::{app::Workspace, relay_transport::RelayTransport};
use futures::future::poll_fn;
use gpui_kit::{Context, Task};
use std::{
    cell::RefCell,
    future::Future,
    pin::Pin,
    rc::{Rc, Weak},
    task::{Poll, Waker},
};
use vtr_query::{Budget, rpc_session::Incarnation};
use wasm_bindgen::{JsCast, JsValue};
thread_local! {
    static ACTIVE: RefCell<Weak<RefCell<RelayTransport>>> = const { RefCell::new(Weak::new()) };
}
pub(crate) struct Host {
    transport: Rc<RefCell<RelayTransport>>,
    incarnation: String,
    wake: Rc<RefCell<Option<Waker>>>,
    _task: Task<()>,
}
impl Host {
    pub fn wake(&self) {
        if let Some(w) = self.wake.borrow_mut().take() {
            w.wake();
        }
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        self.transport.borrow_mut().fail();
        let _ = post("rpcClose", &self.incarnation, None, None);
    }
}
fn identity(value: &str) -> Result<Incarnation, JsValue> {
    if value.len() != 32
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(JsValue::from_str("invalid relay incarnation"));
    }
    let mut bytes = [0; 16];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[i * 2..i * 2 + 2], 16).unwrap();
    }
    Ok(Incarnation(bytes))
}
fn token(value: &str) -> Result<u64, JsValue> {
    let number = value
        .parse::<u64>()
        .map_err(|_| JsValue::from_str("invalid courier token"))?;
    if number == 0 || number.to_string() != value {
        return Err(JsValue::from_str("invalid courier token"));
    }
    Ok(number)
}
fn post(
    kind: &str,
    incarnation: &str,
    token: Option<u64>,
    bytes: Option<&[u8]>,
) -> Result<(), JsValue> {
    let message = js_sys::Object::new();
    js_sys::Reflect::set(&message, &"type".into(), &kind.into())?;
    js_sys::Reflect::set(&message, &"incarnation".into(), &incarnation.into())?;
    if let Some(token) = token {
        js_sys::Reflect::set(&message, &"token".into(), &token.to_string().into())?;
    }
    if let Some(bytes) = bytes {
        js_sys::Reflect::set(
            &message,
            &"bytes".into(),
            &js_sys::Uint8Array::from(bytes).buffer(),
        )?;
    }
    let function = js_sys::Reflect::get(&js_sys::global(), &"volnaRpc".into())?
        .dyn_into::<js_sys::Function>()?;
    function.call1(&JsValue::NULL, &message)?;
    Ok(())
}
pub fn written(incarnation: &str, courier: &str) -> Result<(), JsValue> {
    let id = identity(incarnation)?;
    let courier = token(courier)?;
    ACTIVE.with(|active| {
        if let Some(transport) = active.borrow().upgrade() {
            transport
                .borrow_mut()
                .written(id, courier)
                .map_err(|e| JsValue::from_str(&e.to_string()))?;
        }
        Ok(())
    })
}
pub fn receive(incarnation: &str, courier: &str, bytes: &[u8]) -> Result<(), JsValue> {
    let id = identity(incarnation)?;
    let courier = token(courier)?;
    let consumed = ACTIVE
        .with(|active| {
            active
                .borrow()
                .upgrade()
                .map(|transport| transport.borrow_mut().receive(id, courier, bytes))
                .transpose()
        })
        .map_err(|e| JsValue::from_str(&e.to_string()))?
        .unwrap_or(false);
    if consumed {
        post("rpcConsumed", incarnation, Some(courier), None)?;
    }
    Ok(())
}
pub fn failed(incarnation: &str) -> Result<(), JsValue> {
    let id = identity(incarnation)?;
    ACTIVE.with(|active| {
        if let Some(transport) = active.borrow().upgrade() {
            transport.borrow_mut().fail_incarnation(id);
        }
    });
    Ok(())
}
pub fn start(
    ws: &mut Workspace,
    name: String,
    incarnation: String,
    uri: String,
    cx: &mut Context<Workspace>,
) -> Result<(), JsValue> {
    let id = identity(&incarnation)?;
    let (transport, opening) = RelayTransport::new(id, Budget::new(256 << 20))
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    ws.web_relay = None;
    ws.queries = None;
    let transport = Rc::new(RefCell::new(transport));
    ACTIVE.with(|active| *active.borrow_mut() = Rc::downgrade(&transport));
    let wake = Rc::new(RefCell::new(None));
    let task_transport = transport.clone();
    let task_wake = wake.clone();
    let task_incarnation = incarnation.clone();
    let opening_generation = ws.app.doc.generation();
    let task = cx.spawn(async move |this, cx| {
        let mut opening = Some(opening);
        let mut pending = None;
        let mut attached = None;
        let budget = Budget::new(128 << 20);
        poll_fn(|task_cx| {
            *task_wake.borrow_mut() = Some(task_cx.waker().clone());
            let result = this.update(cx, |ws, cx| -> Result<Poll<()>, String> {
                if opening.is_some() && ws.app.doc.generation() != opening_generation {
                    return Ok(Poll::Ready(()));
                }
                if let Some(future) = &mut opening
                    && let Poll::Ready(result) = Pin::new(future).poll(task_cx)
                {
                    opening = None;
                    let session = result.map_err(|e| e.to_string())?;
                    ws.app
                        .open_query_resource(
                            name.clone(),
                            session.info(),
                            65536,
                            &budget,
                            uri.clone(),
                        )
                        .map_err(|e| e.to_string())?;
                    pending = Some(session);
                    ws.after(None, cx);
                }
                if pending
                    .as_ref()
                    .is_some_and(|s| ws.app.doc.query_snapshot() == Some(s.info().snapshot))
                {
                    let session = pending.take().unwrap();
                    attached = Some(session.info().snapshot);
                    ws.attach_queries(session, cx);
                    ws.after(None, cx);
                }
                if pending
                    .as_ref()
                    .is_some_and(|s| !ws.app.is_query_resource_pending(s.info().snapshot))
                {
                    return Ok(Poll::Ready(()));
                }
                if attached.is_some() && ws.app.doc.query_snapshot() != attached {
                    return Ok(Poll::Ready(()));
                }
                Ok(Poll::Pending)
            });
            let progress = match result {
                Ok(Ok(progress)) => progress,
                Ok(Err(error)) => {
                    let _ = this.update(cx, |ws, cx| {
                        ws.app
                            .report_workspace_error(format!("Trace query failed: {error}"));
                        ws.after(None, cx);
                    });
                    task_transport.borrow_mut().fail();
                    let _ = post("rpcClose", &task_incarnation, None, None);
                    return Poll::Ready(());
                }
                Err(_) => return Poll::Ready(()),
            };
            if progress.is_ready() {
                task_transport.borrow_mut().fail();
                let _ = post("rpcClose", &task_incarnation, None, None);
                return progress;
            }
            let packet = task_transport.borrow_mut().poll_send(task_cx);
            match packet {
                Poll::Ready(Ok(Some(token))) => {
                    let sent = post(
                        "rpcSend",
                        &task_incarnation,
                        Some(token),
                        task_transport.borrow().packet(),
                    );
                    if let Err(error) = sent {
                        task_transport.borrow_mut().fail();
                        let _ = post("rpcClose", &task_incarnation, None, None);
                        let _ = this.update(cx, |ws, cx| {
                            ws.app.report_workspace_error(format!(
                                "Cannot send trace query: {error:?}"
                            ));
                            ws.after(None, cx);
                        });
                        return Poll::Ready(());
                    }
                }
                Poll::Ready(Ok(None) | Err(_)) => {
                    let _ = this.update(cx, |ws, cx| {
                        ws.app
                            .report_workspace_error("Trace query connection closed".into());
                        ws.after(None, cx);
                    });
                    let _ = post("rpcClose", &task_incarnation, None, None);
                    return Poll::Ready(());
                }
                Poll::Pending => {}
            }
            Poll::Pending
        })
        .await;
    });
    ws.web_relay = Some(Host {
        transport,
        incarnation,
        wake,
        _task: task,
    });
    Ok(())
}
