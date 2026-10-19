/* An example plugin (include/salir.h): a turning triangle, drawn with OpenGL 3.3
 * as libmpv's render API would draw a video, a red bar along its top (to see it's
 * the right way up). The page sets its speed ({"speed": 2}, or the element's
 * speed="2"), and is told how many
 * frames it's drawn ({"frames": n}) every second's worth; its input is logged.
 *
 *   cc -O2 -shared -fPIC -Iinclude -o libtriangle.so examples/triangle/triangle.c
 *   salir --plugin triangle=./libtriangle.so page.html */
#include "salir.h"
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef unsigned int GLuint;
typedef int GLint;
typedef unsigned int GLenum;
typedef float GLfloat;
#define GL_COLOR_BUFFER_BIT 0x4000
#define GL_SCISSOR_TEST 0x0C11
#define GL_TRIANGLES 0x0004
#define GL_FRAMEBUFFER 0x8D40
#define GL_VERTEX_SHADER 0x8B31
#define GL_FRAGMENT_SHADER 0x8B30

struct gl {
  void (*BindFramebuffer)(GLenum, GLuint);
  void (*Viewport)(GLint, GLint, GLint, GLint);
  void (*ClearColor)(GLfloat, GLfloat, GLfloat, GLfloat);
  void (*Clear)(GLenum);
  void (*Enable)(GLenum);
  void (*Disable)(GLenum);
  void (*Scissor)(GLint, GLint, GLint, GLint);
  GLuint (*CreateShader)(GLenum);
  void (*ShaderSource)(GLuint, int, const char *const *, const GLint *);
  void (*CompileShader)(GLuint);
  GLuint (*CreateProgram)(void);
  void (*AttachShader)(GLuint, GLuint);
  void (*LinkProgram)(GLuint);
  void (*UseProgram)(GLuint);
  GLint (*GetUniformLocation)(GLuint, const char *);
  void (*Uniform1f)(GLint, GLfloat);
  void (*Uniform2f)(GLint, GLfloat, GLfloat);
  void (*GenVertexArrays)(int, GLuint *);
  void (*BindVertexArray)(GLuint);
  void (*DrawArrays)(GLenum, GLint, int);
  void (*DeleteProgram)(GLuint);
  void (*DeleteVertexArrays)(int, const GLuint *);
};

struct triangle {
  const salir_host *host;
  struct gl gl;
  GLuint program, vao;
  GLint angle_at, scale_at;
  float angle, speed;
  long frames;
};

static const char *VS =
    "#version 330 core\n"
    "uniform float angle;\n"
    "uniform vec2 scale; /* (its own shape whatever the element's) */\n"
    "out vec3 colour;\n"
    "void main() {\n"
    "  float a = angle + float(gl_VertexID) * 2.0943951;\n"
    "  gl_Position = vec4(0.8 * vec2(sin(a), cos(a)) * scale, 0.0, 1.0);\n"
    "  colour = vec3(gl_VertexID == 0, gl_VertexID == 1, gl_VertexID == 2);\n"
    "}\n";
static const char *FS =
    "#version 330 core\n"
    "in vec3 colour;\n"
    "out vec4 frag;\n"
    "void main() { frag = vec4(colour, 1.0); }\n";

/* A number after `key` in JSON: "speed": 2, or "speed": "2" (an attribute's). */
static int number(const char *json, const char *key, float *out) {
  const char *at = strstr(json, key);
  if (!at || !(at = strchr(at + strlen(key), ':'))) return 0;
  at += 1 + strspn(at + 1, " \"");
  *out = (float)atof(at);
  return 1;
}

static GLuint shader(struct gl *gl, GLenum type, const char *src) {
  GLuint s = gl->CreateShader(type);
  gl->ShaderSource(s, 1, &src, NULL);
  gl->CompileShader(s);
  return s;
}

static void *create(const salir_host *host, const char *options) {
  struct triangle *t = calloc(1, sizeof *t);
  t->host = host;
  t->speed = 1.0f;
  number(options, "\"speed\"", &t->speed);
#define LOAD(name) *(void **)&t->gl.name = host->get_proc_address(host->ctx, "gl" #name)
  LOAD(BindFramebuffer); LOAD(Viewport); LOAD(ClearColor); LOAD(Clear); LOAD(Enable); LOAD(Disable);
  LOAD(Scissor); LOAD(CreateShader); LOAD(ShaderSource); LOAD(CompileShader); LOAD(CreateProgram);
  LOAD(AttachShader); LOAD(LinkProgram); LOAD(UseProgram); LOAD(GetUniformLocation); LOAD(Uniform1f); LOAD(Uniform2f);
  LOAD(GenVertexArrays); LOAD(BindVertexArray); LOAD(DrawArrays); LOAD(DeleteProgram); LOAD(DeleteVertexArrays);
  if (!t->gl.DrawArrays || !t->gl.GenVertexArrays) return free(t), NULL;
  t->program = t->gl.CreateProgram();
  t->gl.AttachShader(t->program, shader(&t->gl, GL_VERTEX_SHADER, VS));
  t->gl.AttachShader(t->program, shader(&t->gl, GL_FRAGMENT_SHADER, FS));
  t->gl.LinkProgram(t->program);
  t->angle_at = t->gl.GetUniformLocation(t->program, "angle");
  t->scale_at = t->gl.GetUniformLocation(t->program, "scale");
  t->gl.GenVertexArrays(1, &t->vao);
  return t;
}

static void render(void *embed, uint32_t fbo, int width, int height) {
  struct triangle *t = embed;
  struct gl *gl = &t->gl;
  gl->BindFramebuffer(GL_FRAMEBUFFER, fbo);
  gl->Viewport(0, 0, width, height);
  gl->ClearColor(0.05f, 0.06f, 0.10f, 1.0f);
  gl->Clear(GL_COLOR_BUFFER_BIT);
  gl->Enable(GL_SCISSOR_TEST); /* the red bar: at the top (GL's y goes up) */
  gl->Scissor(0, height - height / 12, width, height / 12);
  gl->ClearColor(0.85f, 0.15f, 0.15f, 1.0f);
  gl->Clear(GL_COLOR_BUFFER_BIT);
  gl->Disable(GL_SCISSOR_TEST);
  gl->UseProgram(t->program);
  gl->Uniform1f(t->angle_at, t->angle);
  /* Clip space is the element's shape: squeezed back along its longer side. */
  gl->Uniform2f(t->scale_at, width > height ? (float)height / width : 1.0f, height > width ? (float)width / height : 1.0f);
  gl->BindVertexArray(t->vao);
  gl->DrawArrays(GL_TRIANGLES, 0, 3);
  t->angle += 0.02f * t->speed;
  if (++t->frames % 60 == 0) {
    char msg[64];
    snprintf(msg, sizeof msg, "{\"frames\": %ld}", t->frames);
    t->host->send(t->host->ctx, msg);
  }
  t->host->request_frame(t->host->ctx); /* (turning: drawn again next frame) */
}

static void message(void *embed, const char *json) {
  struct triangle *t = embed;
  /* {"speed": 2}, or the element's attribute changed: {"attribute": {"name": "speed", "value": "2"}} */
  if (strstr(json, "\"attribute\"")) {
    if (strstr(json, "\"name\":\"speed\"") || strstr(json, "\"name\": \"speed\"")) number(json, "\"value\"", &t->speed);
  } else if (!number(json, "\"speed\"", &t->speed) && strstr(json, "\"input\"")) fprintf(stderr, "triangle: %s\n", json);
}

static void destroy(void *embed) {
  struct triangle *t = embed;
  t->gl.DeleteProgram(t->program);
  t->gl.DeleteVertexArrays(1, &t->vao);
  free(t);
}

static const salir_plugin PLUGIN = {SALIR_ABI, create, render, message, destroy};

const salir_plugin *salir_plugin_v1(void) { return &PLUGIN; }
