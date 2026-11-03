//! Gives the Windows executable its icon. Other platforms set it elsewhere.
fn main() {
    println!("cargo:rerun-if-changed=assets/linecon.rc");
    println!("cargo:rerun-if-changed=assets/linecon.ico");
    #[cfg(windows)]
    embed_resource::compile("assets/linecon.rc", embed_resource::NONE)
        .manifest_optional()
        .expect("could not embed the icon");
}
