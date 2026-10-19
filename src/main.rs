//! salir [options] [URL or file]
//!
//!   --plugin KIND=PATH   embeds of KIND from a plugin (include/salir.h); again for more
//!   --control            driven through stdin/stdout, JSON lines (src/control.rs)
//!   --direct             the page in a window of its own, drawn and shown by Chromium:
//!                        no compositing, no embeds (a WebGPU game, an app's UI)
//!   --title TITLE        the window's (else the page's)
//!   --size WxH           in CSS pixels (1280x800)
//!   --app-id ID          its Wayland app id, for window rules
//!   --name NAME          its data's folder: ~/.cache/NAME (salir)
//!   --data DIR           its data's folder, any (CEF's profile: cache, cookies, storage)
//!   --trust-certificate FILE   a certificate (PEM) whose errors are let through: that one alone

use salir::{Salir, WindowOptions};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // CEF's other processes are this program too, their arguments CEF's.
    if args.iter().any(|a| a.starts_with("--type=")) {
        return Salir::new("salir").run();
    }
    let mut salir = Salir::new(value(&args, "--name").unwrap_or("salir"));
    if let Some(dir) = value(&args, "--data") {
        salir = salir.data(dir);
    }
    if let Some(file) = value(&args, "--trust-certificate") {
        match std::fs::read_to_string(file) {
            Ok(pem) => salir = salir.trust_certificate(&pem),
            Err(e) => return usage(&format!("--trust-certificate {file}: {e}")),
        }
    }
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--plugin" {
            match args.get(i + 1).and_then(|p| p.split_once('=')) {
                Some((kind, path)) => salir = salir.plugin(kind, path),
                None => return usage("--plugin KIND=PATH"),
            }
            i += 1;
        }
        i += 1;
    }
    let control = args.iter().any(|a| a == "--control");
    let url = args.iter().enumerate().filter(|(i, a)| !a.starts_with("--") && !(*i > 0 && ["--plugin", "--title", "--size", "--app-id", "--name", "--data", "--trust-certificate"].contains(&args[i - 1].as_str()))).map(|(_, a)| a).next_back();
    match url {
        Some(url) => {
            let mut w = WindowOptions::new("main", to_url(url));
            w.title = value(&args, "--title").map(str::to_owned);
            w.app_id = value(&args, "--app-id").map(str::to_owned);
            w.direct = args.iter().any(|a| a == "--direct");
            if let Some((width, height)) = value(&args, "--size").and_then(|s| s.split_once('x')).and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?))) {
                (w.width, w.height) = (width, height);
            }
            salir = salir.window(w);
        }
        None if !control => return usage("a URL or a file (or --control)"),
        None => {}
    }
    salir.control(control).run()
}

fn value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

// A file's path as a file:// URL; a URL as it is.
fn to_url(s: &str) -> String {
    if s.contains("://") || s.starts_with("about:") || s.starts_with("data:") {
        return s.to_string();
    }
    match std::fs::canonicalize(s) {
        Ok(p) => format!("file://{}", p.display()),
        Err(_) => s.to_string(),
    }
}

fn usage(what: &str) -> ExitCode {
    eprintln!("salir: {what}\nusage: salir [--plugin KIND=PATH]... [--control] [--direct] [--title T] [--size WxH] [--app-id ID] [--name NAME] [--data DIR] [--trust-certificate FILE] [URL or file]");
    ExitCode::from(2)
}
