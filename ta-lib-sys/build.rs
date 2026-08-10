use bindgen::{
    callbacks::{EnumVariantValue, ParseCallbacks},
    Builder,
};
use std::{env, path::PathBuf, process::Command};

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

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=wrapper.h");

    // Track only the parts of bundled TA-Lib that affect compilation,
    // not the whole `ta-lib-0.4.0/` directory. autotools regenerates
    // `Makefile`, `config.status`, `config.log`, `libtool`, etc. on every
    // `./configure`, and `make install` mutates build artifacts under
    // `src/.libs/`, `bin`, `*.la` files, etc. If we tracked the whole
    // directory cargo would see those changes, rerun the build script
    // on every `cargo check`, and `./configure && make install` would
    // re-touch the same files -- a self-sustaining loop that re-compiled
    // ~114k LOC of TA-Lib on every check.
    for entry in std::fs::read_dir(format!("{TA_LIB_PATH}/include"))
        .expect("read bundled include dir")
        .flatten()
    {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Skip autotools-generated files: `Makefile.in`, the configure
        // sentinel `stamp-h1`, and `ta_config.h` (generated from
        // `ta_config.h.in` by `./configure`).
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
                // Skip build artifacts (`.libs/`, generated `Makefile`,
                // `Makefile.in`, libtool metadata `*.la`, object files
                // `*.o` / `*.lo`). We only want sources.
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
    // The autotools build config -- if these change, reconfigure is needed.
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

        /*
        let ta_lib = autotools::Config::new(TA_LIB_PATH)
            .enable_static()
            .insource(true)
            .build();

        // Simply link the library without using pkg-config
        println!("cargo:rustc-link-search=native={}", ta_lib.display());
        println!("cargo:rustc-link-lib=static=ta-lib");

        eprintln!("{}", ta_lib.display());
        */

        // Only (re)run ./configure if the tree isn't already configured
        // for the current --prefix. Without this, every build script
        // invocation regenerates `Makefile`, `config.status`, `config.log`,
        // `libtool`, `include/stamp-h1`, `ta-lib.spec`, `ta-lib.dpkg`,
        // `ta-lib-config`, etc. Those regenerated files live under
        // `ta-lib-0.4.0/` which is also in `rerun-if-changed`, so cargo
        // sees them as "changed" and reruns the build script again on the
        // next `cargo check` -> the same 10+ second regression the user
        // asked us to fix.
        let prefix = out_path.display().to_string();
        let makefile = std::path::Path::new(TA_LIB_PATH).join("Makefile");
        let already_configured = makefile
            .is_file()
            && std::fs::read_to_string(&makefile)
                .map(|s| {
                    s.lines()
                        .any(|l| l.trim_start().starts_with("prefix = ") && {
                            // `prefix` line is literally `prefix = /path/to/out`
                            let v = l.trim_start().trim_start_matches("prefix = ").trim();
                            v == prefix
                        })
                })
                .unwrap_or(false);

        if !already_configured {
            Command::new("./configure")
                .current_dir(TA_LIB_PATH)
                .arg(format!("--prefix={}", prefix))
                .output()
                .expect("Failed to execute TA C library configure script.");
        }

        let nproc = std::thread::available_parallelism()
            .map(|n| n.get().to_string())
            .unwrap_or_else(|_| "1".to_string());

        Command::new("make")
            .current_dir(TA_LIB_PATH)
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
        // Don't register bindgen::CargoCallbacks: it emits
        // `cargo:rerun-if-changed=` for every transitively-included header,
        // including those under OUT_DIR/include/ta-lib/ that `make install`
        // itself writes. That creates a self-sustaining loop where the
        // build script is re-run on every `cargo check` (cargo sees the
        // OUT_DIR headers as "changed" because we just overwrote them).
        // `wrapper.h` and the relevant TA-Lib sources are tracked explicitly
        // above, so CargoCallbacks is unnecessary and harmful here.
        .parse_callbacks(Box::new(CleanTaNamingCallbacks {}))
        .generate()
        .expect("Unable to generate bindings.");

    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("Could not write bindings.");
}
