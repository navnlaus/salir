// window.salir: a page's way to salir (put in every page; __salirSend is native).
//
//   salir.send(message)                    to the app (--control: stdout; a Rust app: on_message)
//   salir.on(fn)                           from it; returns a function that stops it
//
// Embeds, native content under the page where an element of it is, its size, as
// it moves (followed every frame); the page shows it where it's see-through there
// (the element's children draw over it). Top frame only. As an element:
//
//   <salir-embed kind="video" src="…" input></salir-embed>
//     its size and place: CSS's (display: block unless styled otherwise)
//     its attributes (but kind, input, id, class, style, tabindex): its options, as strings;
//       changed later: a message {"attribute": {"name", "value"}} (value null: removed)
//     kind changed, or the element put elsewhere: made again
//     input: the element's pointer, wheel and keys to it too
//     el.send(message); el.addEventListener("message" / "error", (e) => e.detail)
//
// or from script: salir.embed(element, kind, options) -> { send(message), on(fn), remove() }
//   (options.input: as the attribute).
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

  if (top && !customElements.get("salir-embed")) {
    const RESERVED = new Set(["kind", "input", "id", "class", "style", "tabindex"]); // (tabindex: input's, to have the keys)
    let styled = false;
    customElements.define("salir-embed", class SalirEmbed extends HTMLElement {
      #handle = null;
      #watch = new MutationObserver((changes) => this.#changed(changes));

      connectedCallback() {
        if (!styled) {
          // (A block by default; the page's own CSS wins: this sheet comes first.)
          const sheet = new CSSStyleSheet();
          sheet.replaceSync("salir-embed { display: block; }");
          document.adoptedStyleSheets = [sheet, ...document.adoptedStyleSheets];
          styled = true;
        }
        this.#make();
        this.#watch.observe(this, { attributes: true });
      }

      disconnectedCallback() {
        this.#watch.disconnect();
        this.#handle?.remove();
        this.#handle = null;
      }

      send(message) {
        this.#handle?.send(message);
      }

      #make() {
        this.#handle?.remove();
        this.#handle = null;
        const kind = this.getAttribute("kind");
        if (!kind) return;
        const options = { input: this.hasAttribute("input") };
        for (const a of this.attributes) if (!RESERVED.has(a.name)) options[a.name] = a.value;
        this.#handle = salir.embed(this, kind, options);
        this.#handle.on((m) => this.dispatchEvent(new CustomEvent(m && typeof m === "object" && "error" in m ? "error" : "message", { detail: m })));
      }

      #changed(changes) {
        const names = new Set(changes.map((c) => c.attributeName));
        if (names.has("kind") || names.has("input")) return this.#make();
        for (const name of names) {
          if (!RESERVED.has(name)) this.#handle?.send({ attribute: { name, value: this.getAttribute(name) } });
        }
      }
    });
  }

  if (top) post({ $: "hello" }); // (a new page: the last one's embeds are gone)
})();
