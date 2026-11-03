//! Builds the pinned PuTTY submodule with its own CMake build, then compiles
//! the shim against the same configuration and links everything statically.
use std::env;
use std::path::PathBuf;

/// Same order as PuTTY's own `plink` target.
const PUTTY_LIBS: &[&str] = &[
    "eventloop",
    "noterminal",
    "console",
    "sshclient",
    "otherbackends",
    "settings",
    "network",
    "crypto",
    "utils",
];

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    // dunce avoids Windows extended-length paths, which MSBuild cannot open.
    let putty = dunce::canonicalize(manifest.join("../../vendor/putty"))
        .expect("vendor/putty is missing; run: git submodule update --init --recursive");
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let windows = target_os == "windows";

    println!("cargo:rerun-if-changed=shim");
    println!("cargo:rerun-if-changed=build.rs");
    println!(
        "cargo:rerun-if-changed={}",
        putty.join("LATEST.VER").display()
    );

    // Building plink pulls in every static library the shim links against, and
    // proves that PuTTY's own link line still works at this pin.
    let mut cfg = cmake::Config::new(&putty);
    cfg.build_target("plink")
        .define("PUTTY_GTK_VERSION", "NONE")
        .profile("Release");
    if windows {
        // Must match the Rust side's crt-static, and PuTTY's cmake policy makes
        // this the setting that counts rather than the compiler flags.
        cfg.static_crt(true)
            .define("CMAKE_MSVC_RUNTIME_LIBRARY", "MultiThreaded");
    }
    let out = cfg.build();
    let build = out.join("build");

    // PuTTY puts some libraries in subdirectories, and multi-config generators
    // add a Release level.
    for sub in [
        "",
        "ssh",
        "charset",
        "Release",
        "ssh/Release",
        "charset/Release",
    ] {
        println!(
            "cargo:rustc-link-search=native={}",
            build.join(sub).display()
        );
    }

    let platform = if windows { "windows" } else { "unix" };
    let mut shim = cc::Build::new();
    shim.include("shim")
        .include(&putty)
        .include(putty.join(platform))
        .include(putty.join("terminal"))
        .include(putty.join("charset"))
        .include(build.join("CMakeFiles"))
        .include(&build)
        .define("HAVE_CMAKE_H", None)
        .warnings(false);
    // be_list.c is configured per application through macros named SSH and
    // friends, which must not leak into the other translation units.
    let mut be_list = shim.clone();
    be_list
        .file(putty.join("be_list.c"))
        .define("APPNAME", "linecon0")
        .define("SSH", "1")
        .define("SERIAL", "1")
        .define("OTHERBACKENDS", "1")
        .compile("linecon_be_list");
    shim.file("shim/linecon_shim.c")
        .file("shim/shim_storage.c")
        .file(putty.join("stubs/no-lineedit.c"))
        .compile("linecon_shim");

    for lib in PUTTY_LIBS {
        println!("cargo:rustc-link-lib=static={lib}");
    }
    if windows {
        for lib in [
            "advapi32", "user32", "gdi32", "ws2_32", "shell32", "comdlg32", "imm32", "ole32",
            "kernel32",
        ] {
            println!("cargo:rustc-link-lib={lib}");
        }
    } else {
        println!("cargo:rustc-link-lib=static=charset");
    }
    if target_os == "linux" {
        println!("cargo:rustc-link-lib=dl");
    }
}
