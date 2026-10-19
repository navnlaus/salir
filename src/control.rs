//! `salir --control`: driven by another program (phil's engine, any) through
//! JSON lines, commands on stdin and events on stdout:
//!
//!   {"open": {"id": "w1", "url": "…", "title"?, "width"?, "height"?, "app_id"?, "background"?: [r, g, b, a]}}
//!   {"close": "w1"}
//!   {"send": {"window": "w1", "message": …}}     to the page: salir.on(fn)
//!   {"quit": true}                                every window closed, then salir (stdin closed: the same)
//!
//!   {"ready": true}
//!   {"opened": "w1"}  {"closed": "w1"}
//!   {"message": {"window": "w1", "message": …}}   from the page: salir.send(message)
//!   {"popup": {"window": "w1", "url": "…"}}       window.open() or a target=_blank link (not opened)
//!   {"error": "…"}

use crate::app::Event;
use crate::WindowOptions;
use serde_json::Value;
use std::io::{BufRead, Write};
use winit::event_loop::EventLoopProxy;

pub enum Command {
    Open(WindowOptions),
    Close(String),
    Send { window: String, message: Value },
    Quit,
}

/// Commands read from stdin, as events; its end, Event::ControlClosed.
pub fn listen(proxy: EventLoopProxy<Event>) {
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(&line).map_err(|e| e.to_string()).and_then(|v| parse(&v)) {
                Ok(command) => {
                    let _ = proxy.send_event(Event::Control(command));
                }
                Err(e) => emit(serde_json::json!({ "error": format!("{e}: {line}") })),
            }
        }
        let _ = proxy.send_event(Event::ControlClosed);
    });
}

fn parse(v: &Value) -> Result<Command, String> {
    let s = |v: &Value| v.as_str().map(str::to_owned);
    if let Some(o) = v.get("open") {
        let id = o.get("id").and_then(s).ok_or("open: no id")?;
        let url = o.get("url").and_then(s).ok_or("open: no url")?;
        let mut w = WindowOptions::new(id, url);
        w.title = o.get("title").and_then(s);
        w.app_id = o.get("app_id").and_then(s);
        if let Some(n) = o.get("width").and_then(Value::as_u64) {
            w.width = n as u32;
        }
        if let Some(n) = o.get("height").and_then(Value::as_u64) {
            w.height = n as u32;
        }
        w.direct = o.get("direct").and_then(Value::as_bool).unwrap_or(false);
        if let Some(b) = o.get("background").and_then(|b| serde_json::from_value::<[f64; 4]>(b.clone()).ok()) {
            w.background = b;
        }
        return Ok(Command::Open(w));
    }
    if let Some(id) = v.get("close").and_then(s) {
        return Ok(Command::Close(id));
    }
    if let Some(o) = v.get("send") {
        return Ok(Command::Send { window: o.get("window").and_then(s).ok_or("send: no window")?, message: o.get("message").cloned().unwrap_or(Value::Null) });
    }
    if v.get("quit").is_some() {
        return Ok(Command::Quit);
    }
    Err("not a command".into())
}

/// An event, a line on stdout.
pub fn emit(event: Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{event}");
    let _ = out.flush();
}
