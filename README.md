# salir

Web pages over native GPU content. Each window is drawn with **wgpu**; its page
by **CEF** (Chromium) off screen, each frame handed over as a GPU texture — no
copy through memory; and under the page, **embeds**: native content where an
element of the page is, drawn with wgpu or OpenGL (a video through libmpv's
render API, a game, a 3D view), with the page's HTML over it.

```
 window (wgpu)                                    direct window (--direct)
 ├── embeds    native, where the page's elements   └── page   Chromium draws it and shows
 │             are (<salir-embed>)                            it itself, as a browser does:
 ├── page      CEF, off screen: its frames as                 no compositing, no embeds
 │             GPU textures (dmabufs)
 └── popup     a <select>'s list
```

A game can be either: native, drawn with wgpu in an embed (its HUD in HTML over
it; the window's input straight to it, `input="direct"`), or all Chromium,
WebGPU in the page, in a direct window (nothing of salir's between Chromium and
the screen: `examples/webgpu`).

Linux (x86-64, Wayland or X11) so far; the rest is kept portable (wgpu, winit,
CEF all are) for Windows and macOS to follow.

## Build, install

```sh
make              # CEF (downloaded once, checked, stripped), then salir: dist/
make DEBUG=1      # the same with debug symbols, CEF's (1.4 GB) and salir's
make run          # the demo (examples/demo), with the examples built
make run-webgpu   # examples/webgpu in a direct window
make install      # ~/.local/bin/salir, ~/.local/lib/salir/ (PREFIX=… for elsewhere)
make uninstall
```

Needs Rust, a C compiler (the example plugin), curl, tar, and a Vulkan driver.
CEF is its official prebuilt release (CEF_VERSION in the Makefile), verified
against its SHA-1, only its runtime files kept: 355 MB stripped.

**CEF 156** (beta, for now): the first where shared textures work on NVIDIA
([CEF #4238](https://github.com/chromiumembedded/cef/pull/4238)); with 154 the
frames come empty there. The Rust bindings are [cef-rs](https://github.com/tauri-apps/cef-rs)'s
for CEF 154's API, which CEF 156 still serves (it keeps API versions back to 133);
cef-rs's build script isn't used (`dox`): salir links the CEF it fetched. Its
calls are all in `src/browser.rs`.

## Use

```sh
salir page.html                              # a window with the page
salir --plugin video=libmyvideo.so app.html  # embeds of kind "video" from a plugin
salir --direct game.html                     # Chromium's own window: no compositing, no embeds
salir --control                              # driven by another program (below)
```

Options: `--title`, `--size WxH`, `--app-id ID` (Wayland's, for window rules),
`--name NAME` (its data: `~/.cache/NAME`), `--data DIR` (its data anywhere),
`--trust-certificate FILE` (a certificate, PEM, whose errors are let through:
that one alone — an app's own server's, for HTTPS on 127.0.0.1, say).

### In the page

An embed is an element, sized and placed by CSS like any:

```html
<salir-embed kind="video" src="/films/x.mkv" input style="width: 640px; aspect-ratio: 16 / 9">
  <div class="controls">…</div>   <!-- its children: HTML over the video -->
</salir-embed>
```

- `kind`: which embed (an app's, or a plugin's: `--plugin video=…`). Its other
  attributes (but `input`, `id`, `class`, `style`, `tabindex`) are the embed's
  options, as strings; one changed later reaches it as
  `{"attribute": {"name", "value"}}` (value `null`: removed). `kind` changed, or
  the element moved elsewhere in the page: it's made again.
- `input`: the element's pointer, wheel and key events go to the embed too,
  passed on by the page (`{"input": {"type", "x", "y", "width", "height",
  "button", "key"…}}`, CSS pixels from the element's top left) — for a video's
  controls, say.
- `input="direct"`: the **window's** input goes straight to the embed instead
  of to the page (a game's: no round trip through the page), till the attribute
  is removed or the embed gives it back (`link.release_input()`, Escape say:
  the attribute's removed, a `release` event fires on the element). `lock` too:
  the pointer locked and hidden, the mouse's raw motion given (FPS-style).
- `el.send(message)` to it; `message` events (`e.detail`) from it, `error` if it
  couldn't be made, `release` when it gave the input back.

It's drawn under the page where the element is, following it as the page lays
out and scrolls; the page shows it where the page is see-through there (no
background on the element, nor behind it), so what the page draws there — the
element's children — goes over it. It's a rectangle: border-radius and clip-path
don't cut it (draw the corners over it). salir sends all the window's input to
the page; the page decides what's on top.

From script, the same: `salir.embed(element, kind, options)` → `{ send, on, remove }`.
And messages with the app:

```js
salir.send({ any: "JSON" });              // to the app (--control: stdout)
salir.on((message) => …);                 // from it
salir.close();                            // the page's window closed (its unload handlers run)
```

### Embeds

In a Rust app (salir as a library, `examples/particles.rs`):

```rust
salir::Salir::new("my-app")
    .embed("particles", |options, link| Ok(Box::new(Particles::new(options, link))))  // wgpu: salir::Embed
    .gl_embed("video", |options, link, get_proc| …)                                   // OpenGL: salir::GlEmbed
    .window(salir::WindowOptions::new("main", "file:///…/app.html"))
    .run()
```

`Embed::input` gets the window's input when it has it directly (`salir::Input`:
moves, raw motion, buttons, wheel, keys by place — `"KeyW"` — and by meaning).
`Embed::render` draws into a wgpu texture salir keeps for it, on every system
(wgpu: Vulkan, Direct3D 12, Metal). GL embeds and plugins are **Linux only** (EGL
and dmabufs): for what only comes as OpenGL there, libmpv's render API above
all. `GlEmbed::render`
into a GL framebuffer (OpenGL 3.3 core, salir's EGL context, no window), which
is shared with wgpu as a dmabuf. Either draws when made, resized, or asked to
(`link.request_frame()` — from any thread: libmpv's update callback);
`link.send()` talks to its page.

In any language (Linux), a **plugin**: a shared library exporting `salir_plugin_v1`
(`include/salir.h`, installed with salir) — OpenGL, `create / render / message
/ destroy`, and the host's `get_proc_address / send / request_frame`.
`examples/triangle/triangle.c` is one. libmpv's render API fits it as it is:
`get_proc_address` for `MPV_RENDER_PARAM_OPENGL_INIT_PARAMS`, `request_frame`
from its update callback, `mpv_render_context_render` into the framebuffer given.
salir shows that framebuffer as a window shows GL's (y up), so libmpv would want
`MPV_RENDER_PARAM_FLIP_Y` 1, as for a window (not tried with libmpv yet).

### `--control`

JSON lines, commands on stdin, events on stdout (`src/control.rs`):

```
→ {"open": {"id": "w1", "url": "…", "title"?, "initial_title"?, "width"?, "height"?, "app_id"?, "background"?: [r, g, b, a], "direct"?: true}}
                       (initial_title: the title till the page sets its own: a window rule's to match)
→ {"send": {"window": "w1", "message": …}}        (salir.on in the page)
→ {"close": "w1"}      → {"quit": true}             (or stdin closed)
→ {"trust_certificate": "-----BEGIN CERTIFICATE-----…"}   (as --trust-certificate)
← {"ready": true}  {"opened": "w1"}  {"closed": "w1"}  {"error": "…"}
← {"message": {"window": "w1", "message": …}}       (salir.send in the page)
← {"popup": {"window": "w1", "url": "…"}}           (window.open, target=_blank: not opened)
```

### Debugging

`SALIR_TRACE=1` (what salir does), `SALIR_CONSOLE=1` (pages' console),
`SALIR_LOG=1` or `verbose` (CEF's log, in its cache folder: `chrome_debug.log`),
`SALIR_CHROMIUM_FLAGS="name=value …"` (Chromium's switches).

A direct window's input reaches Chromium as salir does CEF's work, so salir
does it every 4 ms while one is open (every 33 ms at most otherwise, and when
CEF asks), and runs GLib's main loop with it: Chromium hears from the
compositor through it on Linux.

## Tested so far

On a GTX 1080 Ti (NVIDIA 580), in a headless Wayland compositor: pages as GPU
textures; a GL plugin and a wgpu embed under the demo page, their messages both
ways, input passed to an embed; `<select>` popups; text typed with real key
events (`þé` too); `--control` with two windows, direct and composited; 400
embeds made and removed (no file descriptors or memory left behind); resizing;
a direct window with 1 M WebGPU particles at 60 fps; an embed given the
window's keys directly, and giving them back. Not yet: resizing a window, a
mouse moved by hand (the compositor has none), HiDPI, AMD and Intel GPUs,
Windows, macOS, the clipboard. Not handled yet: drag and drop, JavaScript
dialogs (`alert()`), file pickers, IME composition (typed text does go through).
Chrome's background services don't reach Google: no background networking,
component updates, pings or sync (switches), the Optimization Guide's AI models,
the network time check and AI Mode's eligibility switched off (features), and
the component updater's server pointed at a closed local port, for anything that
asks it anyway (its on-demand installs ignore `--disable-component-update`). In
a fresh profile, 90 s: no request but those to that port; no socket opened.
