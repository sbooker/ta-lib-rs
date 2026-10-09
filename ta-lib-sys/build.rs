use bindgen::{
    callbacks::{EnumVariantValue, ParseCallbacks},
    Builder,
};
use std::{env, path::Path, path::PathBuf, process::Command};

#[derive(Debug)]
struct CleanTaNamingCallbacks {}

impl ParseCallbacks for CleanTaNamingCallbacks {
    fn item_name(&self, original_item_name: &str) -> Option<String> {
        if original_item_name.starts_with("TA_") {
            Some(original_item_name.trim_start_matches("TA_").to_string())
        } else {
            None
        }
    }

    fn enum_variant_name(
        &self,
        enum_name: Option<&str>,
        original_variant_name: &str,
        _variant_value: EnumVariantValue,
    ) -> Option<String> {
        if enum_name.is_some() {
            Some(original_variant_name.trim_start_matches("TA_").to_string())
        } else {
            None
        }
    }
}

const TA_LIB_PATH: &str = "ta-lib-0.4.0";

fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> std::io::Result<()> {
    let dst = dst.as_ref();
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dest_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(entry.path(), &dest_path)?;
        } else {
            std::fs::copy(entry.path(), &dest_path)?;
        }
    }
    Ok(())
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=wrapper.h");

    // Track only original bundled source files that affect compilation
    for entry in std::fs::read_dir(format!("{TA_LIB_PATH}/include"))
        .expect("read bundled include dir")
        .flatten()
    {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == "Makefile.in" || name == "Makefile" || name == "stamp-h1"
            || name == "ta_config.h"
        {
            continue;
        }
        println!("cargo:rerun-if-changed={}", entry.path().display());
    }
    for dir in ["src/ta_common", "src/ta_abstract", "src/ta_func"] {
        let p = format!("{TA_LIB_PATH}/{dir}");
        if let Ok(rd) = std::fs::read_dir(&p) {
            for entry in rd.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with('.')
                    || name == "Makefile"
                    || name == "Makefile.in"
                    || name.ends_with(".la")
                    || name.ends_with(".o")
                    || name.ends_with(".lo")
                {
                    continue;
                }
                println!("cargo:rerun-if-changed={}", entry.path().display());
            }
        }
    }

    println!("cargo:rerun-if-changed={TA_LIB_PATH}/configure");
    println!("cargo:rerun-if-changed={TA_LIB_PATH}/configure.in");
    println!("cargo:rerun-if-changed={TA_LIB_PATH}/Makefile.am");
    println!("cargo:rerun-if-changed={TA_LIB_PATH}/src/Makefile.am");
    println!("cargo:rerun-if-changed={TA_LIB_PATH}/include/Makefile.am");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());

    println!("cargo:rustc-link-lib=static=ta_lib");

    if !cfg!(feature = "use_system_lib") {
        println!(
            "cargo:rustc-link-search=native={}",
            out_path.join("lib").display()
        );

        // Copy C source tree to OUT_DIR to keep package source directory untouched
        let build_dir = out_path.join("ta-lib-build");
        if !build_dir.exists() {
            copy_dir_all(TA_LIB_PATH, &build_dir)
                .expect("Failed to copy TA-Lib sources to OUT_DIR");
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let configure_path = build_dir.join("configure");
            if let Ok(meta) = std::fs::metadata(&configure_path) {
                let mut perms = meta.permissions();
                perms.set_mode(perms.mode() | 0o755);
                let _ = std::fs::set_permissions(&configure_path, perms);
            }
        }

        let prefix = out_path.display().to_string();
        let makefile = build_dir.join("Makefile");
        let already_configured = makefile
            .is_file()
            && std::fs::read_to_string(&makefile)
            .map(|s| {
                s.lines()
                    .any(|l| l.trim_start().starts_with("prefix = ") && {
                        let v = l.trim_start().trim_start_matches("prefix = ").trim();
                        v == prefix
                    })
            })
            .unwrap_or(false);

        if !already_configured {
            Command::new("./configure")
                .current_dir(&build_dir)
                .arg(format!("--prefix={}", prefix))
                .output()
                .expect("Failed to execute TA C library configure script.");
        }

        let nproc = std::thread::available_parallelism()
            .map(|n| n.get().to_string())
            .unwrap_or_else(|_| "1".to_string());

        Command::new("make")
            .current_dir(&build_dir)
            .arg(format!("-j{nproc}"))
            .arg("install")
            .output()
            .expect("Failed to build TA C library.");
    }

    let bindings = Builder::default()
        .header("wrapper.h")
        .clang_arg(format!("-I{}", out_path.join("include").display()))
        .allowlist_function("TA_.*")
        .allowlist_type("TA_.*")
        .allowlist_var("TA_.*")
        .rustified_enum("TA_.*")
        .prepend_enum_name(false)
        .parse_callbacks(Box::new(CleanTaNamingCallbacks {}))
        .generate()
        .expect("Unable to generate bindings.");

    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("Could not write bindings.");
}