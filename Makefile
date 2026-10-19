# salir: `make` fetches CEF (once) and builds; `make install` puts it in ~/.local.
#
#   make                 CEF, then salir, put together in dist/ (runs from there)
#   make DEBUG=1         the same with debug symbols: CEF's own (1.4 GB) and salir's
#   make examples        the example plugin and embed (dist/plugins, dist/salir-particles)
#   make run             the demo page, with the examples
#   make install         ./install.sh (PREFIX=~/.local)
#   make clean           salir's build; `make distclean` CEF too
#
# CEF is Chromium as a library: its official prebuilt binaries (building Chromium
# takes hours and 100+ GB), checked against their SHA-1. 156 (beta): the first with
# shared textures working on NVIDIA (CEF #4238). Linux x86-64 only, so far.

CEF_VERSION  := 156.0.7+g47dbc1d+chromium-156.0.8078.17
CEF_ARCHIVE  := cef_binary_$(CEF_VERSION)_linux64_beta_minimal.tar.bz2
CEF_SHA1     := 7e9c0ee0e22ecfb3a513387def201b016fc89c84
CEF_URL      := https://cef-builds.spotifycdn.com/$(subst +,%2B,$(CEF_ARCHIVE))

DEBUG  ?= 0
PREFIX ?= $(HOME)/.local
ifeq ($(DEBUG),1)
  CEF_DIR := cef/$(firstword $(subst +, ,$(CEF_VERSION)))-debug
  PROFILE := debuginfo
else
  CEF_DIR := cef/$(firstword $(subst +, ,$(CEF_VERSION)))
  PROFILE := release
endif
export SALIR_CEF_DIR := $(abspath $(CEF_DIR))
CARGO_OUT := target/$(PROFILE)

.PHONY: all cef build examples run install uninstall clean distclean
all: build

# CEF: downloaded once (cef/downloads), its runtime files only (Release/, Resources/);
# stripped of debug symbols unless DEBUG=1.
cef: $(CEF_DIR)/libcef.so
cef/downloads/$(CEF_ARCHIVE):
	mkdir -p cef/downloads
	curl -fL --progress-bar -o $@.part '$(CEF_URL)'
	echo '$(CEF_SHA1)  $@.part' | sha1sum -c --quiet
	mv $@.part $@
$(CEF_DIR)/libcef.so: cef/downloads/$(CEF_ARCHIVE)
	rm -rf $(CEF_DIR) $(CEF_DIR).part && mkdir -p $(CEF_DIR).part
	tar -xjf $< -C $(CEF_DIR).part --strip-components=2 --wildcards '*/Release/*' '*/Resources/*'
ifneq ($(DEBUG),1)
	strip --strip-unneeded $(CEF_DIR).part/*.so*
endif
	mv $(CEF_DIR).part $(CEF_DIR)

# salir, and dist/: it with CEF's files beside it (linked: they're found beside the program).
build: cef
	cargo build --profile $(PROFILE) --bin salir
	mkdir -p dist
	find dist -maxdepth 1 -type l -delete
	for f in $(SALIR_CEF_DIR)/*; do ln -s "$$f" dist/; done
	cp $(CARGO_OUT)/salir dist/

examples: build
	mkdir -p dist/plugins
	$(CC) -O2 -Wall -shared -fPIC -Iinclude -o dist/plugins/libtriangle.so examples/triangle/triangle.c
	cargo build --profile $(PROFILE) --example particles
	cp $(CARGO_OUT)/examples/particles dist/salir-particles

run: examples
	dist/salir-particles --plugin triangle=dist/plugins/libtriangle.so examples/demo/index.html

install: build
	PREFIX='$(PREFIX)' ./install.sh

uninstall:
	PREFIX='$(PREFIX)' ./install.sh --uninstall

clean:
	cargo clean
	rm -rf dist

distclean: clean
	rm -rf cef
