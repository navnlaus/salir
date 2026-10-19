// window.salir: a page's way to salir (put in every page; __salirSend is native).
//
//   salir.send(message)                    to the app (--control: stdout; a Rust app: on_message)
//   salir.on(fn)                           from it; returns a function that stops it
//   salir.embed(element, kind, options)    native content where `element` is, under the page:
//     -> { send(message), on(fn), remove() }   options.input: the element's input to it too
//
// An embed is drawn where the element is, its size, as it moves (followed every
// frame); the page shows it where it's see-through there. Top frame only.
(() => {
  if (globalThis.salir) return;
  const native = globalThis.__salirSend;
  delete globalThis.__salirSend;
  const post = (m) => native(JSON.stringify(m));
  const top = globalThis === globalThis.top;
  const listeners = new Set();
  const embeds = new Map(); // id -> { element, handlers, last }
  let nextId = 1, following = false;

  // Where each embed's element is (CSS pixels, from the window's top left), told when it changes.
  function follow() {
    following = embeds.size > 0;
    if (!following) return;
    for (const [id, e] of embeds) {
      const r = e.element.getBoundingClientRect();
      const visible = e.element.isConnected && r.width > 0 && r.height > 0 && getComputedStyle(e.element).visibility !== "hidden";
      const now = visible ? [r.left, r.top, r.width, r.height] : null;
      const key = JSON.stringify(now);
      if (key !== e.last) post({ $: "embed.rect", id, rect: now }), (e.last = key);
    }
    requestAnimationFrame(follow);
  }

  const INPUT = ["pointerdown", "pointermove", "pointerup", "pointercancel", "wheel", "keydown", "keyup", "dblclick"];
  function forward(id, element) {
    const off = [];
    for (const type of INPUT) {
      const fn = (ev) => {
        const r = element.getBoundingClientRect();
        const input = { type, x: ev.clientX - r.left, y: ev.clientY - r.top, width: r.width, height: r.height, button: ev.button, buttons: ev.buttons, deltaX: ev.deltaX, deltaY: ev.deltaY, key: ev.key, code: ev.code, shift: ev.shiftKey, ctrl: ev.ctrlKey, alt: ev.altKey };
        for (const k of Object.keys(input)) if (input[k] === undefined || Number.isNaN(input[k])) delete input[k];
        post({ $: "embed.message", id, message: { input } });
        if (type === "pointerdown") {
          try {
            element.setPointerCapture(ev.pointerId); // (its moves and release, even off it)
          } catch { /* not a real pointer's */ }
          element.focus();
        }
      };
      element.addEventListener(type, fn);
      off.push(() => element.removeEventListener(type, fn));
    }
    if (element.tabIndex < 0) element.tabIndex = 0; // (focusable: its keys)
    return () => off.forEach((f) => f());
  }

  globalThis.salir = Object.freeze({
    send(message) {
      post({ $: "message", message });
    },
    on(fn) {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
    embed(element, kind, options = {}) {
      if (!top) throw new Error("salir.embed: in the top frame only");
      const id = nextId++;
      const e = { element, handlers: new Set(), last: "" };
      embeds.set(id, e);
      post({ $: "embed.create", id, kind, options });
      const stopInput = options.input ? forward(id, element) : () => {};
      if (!following) requestAnimationFrame(follow), (following = true);
      return Object.freeze({
        id,
        send: (message) => post({ $: "embed.message", id, message }),
        on(fn) {
          e.handlers.add(fn);
          return () => e.handlers.delete(fn);
        },
        remove() {
          if (!embeds.delete(id)) return;
          stopInput();
          post({ $: "embed.remove", id });
        },
      });
    },
    // From salir.
    _receive(m) {
      if (m.$ === "message") for (const fn of listeners) fn(m.message);
      else if (m.$ === "embed.message") for (const fn of embeds.get(m.id)?.handlers ?? []) fn(m.message);
      else if (m.$ === "embed.error") {
        console.error(`salir.embed ${m.id}: ${m.error}`);
        for (const fn of embeds.get(m.id)?.handlers ?? []) fn({ error: m.error });
      }
    },
  });
  if (top) post({ $: "hello" }); // (a new page: the last one's embeds are gone)
})();
