# salir

Web pages over native GPU content. Each window is drawn with **wgpu**; its page
by **CEF** (Chromium) off screen, each frame handed over as a GPU texture — no
copy through memory; and under the page, **embeds**: native content where an
element of the page is, drawn with wgpu or OpenGL (a video through libmpv's
render API, a game, a 3D view), with the page's HTML over it.

```
 window (wgpu)
 ├── embeds    native, where the page's elements are (salir.embed)
 ├── page      CEF, off screen: its frames as GPU textures (dmabufs)
 └── popup     a <select>'s list
```

Linux (x86-64, Wayland or X11) so far; the rest is kept portable (wgpu, winit,
CEF all are) for Windows and macOS to follow.

## Build, install

```sh
make              # CEF (downloaded once, checked, stripped), then salir: dist/
make DEBUG=1      # the same with debug symbols, CEF's (1.4 GB) and salir's
make run          # the demo (examples/demo), with the examples built
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
salir --control                              # driven by another program (below)
```

Options: `--title`, `--size WxH`, `--app-id ID` (Wayland's, for window rules),
`--name NAME` (its data: `~/.cache/NAME`).

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
- `input`: the element's pointer, wheel and key events go to the embed too
  (`{"input": {"type", "x", "y", "width", "height", "button", "key"…}}`, CSS
  pixels from the element's top left).
- `el.send(message)` to it; `message` events (`e.detail`) from it, `error` if it
  couldn't be made.

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
→ {"open": {"id": "w1", "url": "…", "title"?, "width"?, "height"?, "app_id"?, "background"?: [r, g, b, a]}}
→ {"send": {"window": "w1", "message": …}}        (salir.on in the page)
→ {"close": "w1"}      → {"quit": true}             (or stdin closed)
← {"ready": true}  {"opened": "w1"}  {"closed": "w1"}  {"error": "…"}
← {"message": {"window": "w1", "message": …}}       (salir.send in the page)
← {"popup": {"window": "w1", "url": "…"}}           (window.open, target=_blank: not opened)
```

### Debugging

`SALIR_TRACE=1` (what salir does), `SALIR_CONSOLE=1` (pages' console),
`SALIR_LOG=1` (CEF's log), `SALIR_CHROMIUM_FLAGS="name=value …"` (Chromium's switches).

## Tested so far

On a GTX 1080 Ti (NVIDIA 580), in a headless Wayland compositor: pages as GPU
textures; a GL plugin and a wgpu embed under the demo page, their messages both
ways, input passed to an embed; `<select>` popups; text typed with real key
events (`þé` too); `--control` with two windows; 400 embeds made and removed
(no file descriptors or memory left behind). Not yet: resizing a window, a
mouse moved by hand (the compositor has none), HiDPI, AMD and Intel GPUs,
Windows, macOS, the clipboard. Not handled yet: drag and drop, JavaScript
dialogs (`alert()`), file pickers, IME composition (typed text does go through).
