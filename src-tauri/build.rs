fn main() {
    // Stamp the build (git sha + date) for the About box.
    shell_core::build_stamp();

    // Materialize the shared release scripts from the pinned shell-core rev. Git-ignored here — a
    // plain clone regenerates them, so there is no second tracked copy to drift. NEVER edit the
    // generated scripts in this repo; edit them in shell-core.
    shell_core::materialize_scripts(std::path::Path::new("../scripts"))
        .expect("materialize shell-core scripts");

    // Materialize the shared chrome into frontendDist (../src) so generate_context! embeds it.
    // The generated files are git-ignored — reproducible from the pinned chrome-core rev + this
    // recipe, so a plain clone still builds (cargo fetches chrome-core; this writes it out).
    let src = std::path::Path::new("../src");
    std::fs::write(src.join("chrome-core.css"), chrome_core::SIDEBAR_CSS)
        .expect("write chrome-core.css");
    std::fs::write(src.join("chrome-core.js"), chrome_core::SIDEBAR_JS)
        .expect("write chrome-core.js");

    tauri_build::build();
}
