// libcef, from the CEF the Makefile fetched (SALIR_CEF_DIR), and looked for beside
// the program when it runs (dist/, or where install.sh put it).
fn main() {
    println!("cargo:rerun-if-env-changed=SALIR_CEF_DIR");
    let dir = std::env::var("SALIR_CEF_DIR").unwrap_or_else(|_| format!("{}/cef/156.0.7", env!("CARGO_MANIFEST_DIR")));
    if !std::path::Path::new(&dir).join("libcef.so").exists() {
        panic!("No CEF in {dir}: `make` fetches it (or set SALIR_CEF_DIR)");
    }
    println!("cargo:rustc-link-search=native={dir}");
    println!("cargo:rustc-link-lib=dylib=cef");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
}
