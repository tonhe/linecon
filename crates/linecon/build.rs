//! Gives the Windows executable its icon and version details. Other platforms set
//! these elsewhere.
fn main() {
    println!("cargo:rerun-if-changed=assets/linecon.ico");
    println!("cargo:rerun-if-env-changed=LINECON_RELEASE");
    #[cfg(windows)]
    windows_resources();
}

#[cfg(windows)]
fn windows_resources() {
    use std::{env, fs, path::Path};
    let release = env::var("LINECON_RELEASE").unwrap_or_default();
    let package = env::var("CARGO_PKG_VERSION").unwrap();
    let version = match release.trim_start_matches('v') {
        "" => format!("{package}-dev"),
        tag => tag.to_string(),
    };
    let numbers = |part: &str| part.parse::<u16>().unwrap_or(0);
    let mut parts = package.split('.');
    let (major, minor, patch) = (
        numbers(parts.next().unwrap_or("0")),
        numbers(parts.next().unwrap_or("0")),
        numbers(parts.next().unwrap_or("0")),
    );
    let manifest = env::var("CARGO_MANIFEST_DIR").unwrap();
    let icon = Path::new(&manifest)
        .join("assets/linecon.ico")
        .display()
        .to_string()
        .replace('\\', "\\\\");
    let rc = format!(
        r#"1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "FileDescription", "linecon0 SSH, Telnet and serial client"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "linecon"
      VALUE "LegalCopyright", "MIT License"
      VALUE "OriginalFilename", "linecon.exe"
      VALUE "ProductName", "linecon0"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    let out = Path::new(&env::var("OUT_DIR").unwrap()).join("linecon.rc");
    fs::write(&out, rc).unwrap();
    embed_resource::compile(&out, embed_resource::NONE)
        .manifest_optional()
        .expect("could not embed the Windows resources");
}
