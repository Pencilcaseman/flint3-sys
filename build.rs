use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{ensure, Context, Result};

fn run(cmd: &mut Command) -> Result<()> {
    let display = format!("{cmd:?}");
    let output = cmd
        .output()
        .context(format!("failed to execute: {display}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "command failed: {display}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    Ok(())
}

fn build_cflags() -> String {
    // Propagate target CPU features into CFLAGS for FLINT's configure.
    let mut flags = String::new();
    if let Ok(f) = std::env::var("CARGO_CFG_TARGET_FEATURE") {
        if f.contains("avx2") {
            flags.push_str(" -mavx2");
        }
    }
    flags
}

fn clone_flint(dest: &Path) -> Result<()> {
    if dest.exists() {
        return Ok(());
    }
    run(Command::new("git")
        .args(["clone", "--recursive", "--depth", "1", "-b", "main"])
        .arg("https://github.com/flintlib/flint.git")
        .arg(dest))
}

fn build_flint(out_dir: &Path) -> Result<()> {
    let flint = out_dir.join("flint");
    clone_flint(&flint)?;

    // Bootstrap (generates configure from autotools)
    if !flint.join("configure").is_file() {
        run(Command::new("sh").current_dir(&flint).arg("./bootstrap.sh"))?;
    }

    // Configure
    if !flint.join("Makefile").is_file() {
        let mut cmd = Command::new("sh");
        cmd.current_dir(&flint)
            .env("CFLAGS", build_cflags())
            .arg("./configure")
            .arg(format!("--prefix={}", out_dir.display()))
            .arg("--disable-shared");

        if cfg!(feature = "gmp-mpfr-sys") {
            cmd.arg(format!(
                "--with-gmp-lib={}",
                std::env::var("DEP_GMP_LIB_DIR")?
            ))
            .arg(format!(
                "--with-gmp-include={}",
                std::env::var("DEP_GMP_INCLUDE_DIR")?
            ));
        }
        run(&mut cmd)?;
    }

    // Build
    if !flint.join("libflint.a").is_file() {
        run(Command::new("make").current_dir(&flint).arg("-j"))?;
    }

    // Install into $OUT_DIR/{lib,include}
    run(Command::new("make").current_dir(&flint).arg("install"))?;

    Ok(())
}

// --- Bindgen (pregenerated) --------------------------------------------------

#[cfg(not(feature = "force-bindgen"))]
fn generate_bindings(out_dir: &Path) -> Result<()> {
    let rs = out_dir.join("flint.rs");
    let c = out_dir.join("flintextern.c");
    println!("cargo::rerun-if-changed={}", rs.display());
    println!("cargo::rerun-if-changed={}", c.display());
    std::fs::copy("./bindgen/flint.rs", &rs)?;
    std::fs::copy("./bindgen/flintextern.c", &c)?;
    Ok(())
}

// --- Bindgen (from source) ---------------------------------------------------

#[cfg(feature = "force-bindgen")]
static SKIP_HEADERS: &[&str] = &[
    "NTL-interface.h",
    "config.h",
    "crt_helpers.h",
    "longlong_asm_clang.h",
    "longlong_asm_gcc.h",
    "longlong_div_gnu.h",
    "longlong_msc_arm64.h",
    "longlong_msc_x86.h",
    "mpfr_mat.h",
    "mpfr_vec.h",
    "gmpcompat.h",
    "fft_small.h",
    "machine_vectors.h",
    "mpn_extras.h",
    "gettimeofday.h",
];

#[cfg(feature = "force-bindgen")]
fn flint_headers(include_dir: &Path) -> Result<Vec<PathBuf>> {
    use std::{collections::HashSet, ffi::OsStr};

    let header_dir = include_dir.join("flint");
    ensure!(header_dir.join("flint.h").is_file(), "cannot find flint.h");

    let skip: HashSet<&OsStr> = SKIP_HEADERS.iter().map(OsStr::new).collect();

    let mut headers = Vec::new();
    for entry in header_dir.read_dir()? {
        let path = entry?.path();
        if path.extension() == Some(OsStr::new("h")) && !skip.contains(path.file_name().unwrap()) {
            headers.push(path);
        }
    }
    Ok(headers)
}

#[cfg(feature = "force-bindgen")]
fn generate_bindings(out_dir: &Path) -> Result<()> {
    use std::io::Write;

    let include_dir = out_dir.join("include");
    let flint_rs = out_dir.join("flint.rs");
    let extern_c = out_dir.join("flintextern.c");
    let extern_tmp = out_dir.join("flintextern-abs.c");

    // Ensure the temp file exists (bindgen won't create it if there are no inline fns)
    std::fs::write(&extern_tmp, b"")?;

    let headers = flint_headers(&include_dir)?;
    let mut builder = bindgen::Builder::default();
    for h in &headers {
        let h = h.to_str().context("non-unicode path")?;
        builder = builder.allowlist_file(h).header(h);
    }

    let bindings = builder
        .derive_default(false)
        .derive_copy(false)
        .derive_debug(false)
        .wrap_static_fns(true)
        .wrap_static_fns_path(&extern_tmp)
        .generate_cstr(true)
        .merge_extern_blocks(true)
        .blocklist_function("__.*")
        .blocklist_var("__.*")
        .rust_target(bindgen::RustTarget::stable(82, 0).ok().unwrap())
        .rust_edition(bindgen::RustEdition::Edition2021)
        .formatter(bindgen::Formatter::Prettyplease)
        .generate()?;

    bindings.write_to_file(&flint_rs)?;

    // Relativise absolute #include paths emitted by bindgen
    let re = regex::Regex::new(r##"^#include\s+".+(flint/[^/]+\.h)""##)?;
    let abs_content = std::fs::read_to_string(&extern_tmp)?;
    let mut out = std::io::BufWriter::new(std::fs::File::create(&extern_c)?);
    for line in abs_content.lines() {
        match re.captures(line) {
            Some(cap) => writeln!(out, r##"#include "{}""##, &cap[1])?,
            None => writeln!(out, "{line}")?,
        }
    }
    drop(out);

    // Optionally save generated bindings back to ./bindgen/ for release
    println!("cargo::rerun-if-env-changed=KEEP_BINDGEN_OUTPUT");
    if std::env::var("KEEP_BINDGEN_OUTPUT").is_ok() {
        std::fs::copy(&flint_rs, "./bindgen/flint.rs")?;
        std::fs::copy(&extern_c, "./bindgen/flintextern.c")?;
    }

    Ok(())
}

fn build_extern(out_dir: &Path) -> Result<()> {
    cc::Build::new()
        .file(out_dir.join("flintextern.c"))
        .include(out_dir.join("include"))
        .flags(["-lflint", "-lmpfr", "-lgmp"])
        .flags([
            "-Wno-old-style-declaration",
            "-Wno-unused-parameter",
            "-Wno-sign-compare",
        ])
        .try_compile("extern")?;
    Ok(())
}

fn main() -> Result<()> {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR")?).canonicalize()?;
    let include_dir = out_dir.join("include");
    let lib_dir = out_dir.join("lib");

    // Step 1: Build FLINT
    build_flint(&out_dir)?;
    ensure!(
        include_dir.join("flint/flint.h").is_file(),
        "missing flint.h after build"
    );
    ensure!(
        lib_dir.join("libflint.a").is_file(),
        "missing libflint.a after build"
    );

    // Link instructions
    println!("cargo::rustc-link-lib=flint");
    println!("cargo::rustc-link-lib=mpfr");
    println!("cargo::rustc-link-lib=gmp");
    println!("cargo::rustc-link-search=native={}", lib_dir.display());
    if cfg!(feature = "gmp-mpfr-sys") {
        println!(
            "cargo::rustc-link-search=native={}",
            std::env::var("DEP_GMP_LIB_DIR")?
        );
    }

    // Step 2: Generate or copy bindings
    generate_bindings(&out_dir)?;
    ensure!(out_dir.join("flint.rs").is_file(), "missing flint.rs");
    ensure!(
        out_dir.join("flintextern.c").is_file(),
        "missing flintextern.c"
    );

    // Step 3: Compile inline function wrappers
    build_extern(&out_dir)?;

    // Export metadata for downstream crates
    println!("cargo::metadata=LIB_DIR={}", lib_dir.display());
    println!("cargo::metadata=INCLUDE_DIR={}", include_dir.display());

    Ok(())
}
