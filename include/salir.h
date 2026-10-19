/* salir's embeds in C (Linux only: EGL, dmabufs): a plugin (a shared library) that draws with OpenGL into a
 * part of a page, under it, where the page puts it: libmpv's render API, a game,
 * anything that draws with GL. Loaded with `salir --plugin NAME=path.so`; a page
 * makes one with salir.embed(element, "NAME", options).
 *
 * Every call but the host's send() and request_frame() is on salir's main thread,
 * with salir's GL context current (OpenGL 3.3 core, or the newest it has; EGL's
 * eglGetProcAddress behind get_proc_address). Draw into the framebuffer render()
 * is given, `width`×`height` pixels, as GL draws (y up: salir turns it the right
 * way); salir waits for it (glFinish) before it shows it. */
#ifndef SALIR_H
#define SALIR_H
#include <stdint.h>

#define SALIR_ABI 1

typedef struct salir_host {
  uint32_t abi; /* SALIR_ABI */
  void *ctx;    /* for the calls below */
  /* GL's and EGL's functions, by name. */
  void *(*get_proc_address)(void *ctx, const char *name);
  /* A message to the page (its embed's handle: handle.on(fn)), JSON. Any thread. */
  void (*send)(void *ctx, const char *json);
  /* render() again, soon (once per frame at most). Any thread: libmpv's update callback. */
  void (*request_frame)(void *ctx);
} salir_host;

typedef struct salir_plugin {
  uint32_t abi; /* SALIR_ABI */
  /* One of it, for an element of a page; `options`: the page's, JSON. NULL: couldn't. */
  void *(*create)(const salir_host *host, const char *options);
  /* Draw a frame into `fbo` (it's the embed's own, kept between frames). */
  void (*render)(void *embed, uint32_t fbo, int width, int height);
  /* A message from the page (handle.send(message)), JSON; input on the element too,
   * if the page asked for it: {"input": {"type": "pointerdown", "x", "y", ...}}. */
  void (*message)(void *embed, const char *json);
  /* Gone (removed from the page, the page or its window closed). */
  void (*destroy)(void *embed);
} salir_plugin;

/* What a plugin exports. */
const salir_plugin *salir_plugin_v1(void);

#endif
