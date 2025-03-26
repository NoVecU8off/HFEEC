// build.rs
use std::env;
use std::path::Path;
use std::process::Command;

fn main() {
    // Получение путей и флагов DPDK
    let dpdk_include_path = Command::new("pkg-config")
        .args(["--cflags", "libdpdk"])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).to_string())
        .unwrap_or_else(|_| "-I/usr/local/include/dpdk".to_string());

    let dpdk_libs = Command::new("pkg-config")
        .args(["--libs", "libdpdk"])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).to_string())
        .unwrap_or_else(|_| {
            "-lrte_eal -lrte_mempool -lrte_ring -lrte_mbuf -lrte_net -lrte_ethdev".to_string()
        });

    println!("DPDK lib flags: {}", dpdk_libs);

    // Стандартные пути для поиска библиотек
    println!("cargo:rustc-link-search=native=/usr/lib");
    println!("cargo:rustc-link-search=native=/usr/local/lib");
    println!("cargo:rustc-link-search=native=/usr/lib/x86_64-linux-gnu");
    println!("cargo:rustc-link-search=native=/usr/lib/dpdk");
    println!("cargo:rustc-link-search=native=/usr/lib/x86_64-linux-gnu/dpdk");

    // Добавление библиотек DPDK
    for lib in dpdk_libs.split_whitespace() {
        if lib.starts_with("-l") {
            println!("cargo:rustc-link-lib={}", &lib[2..]);
        } else if lib.starts_with("-L") {
            println!("cargo:rustc-link-search=native={}", &lib[2..]);
        }
    }

    // Определение, доступны ли huge pages
    let has_hugepages = check_hugepages_available();
    if has_hugepages {
        println!("cargo:rustc-cfg=feature=\"hugepages\"");
    }

    // Проверка, поддерживает ли DPDK опции NUMA
    let has_numa = check_dpdk_numa_support();
    if has_numa {
        println!("cargo:rustc-cfg=feature=\"numa\"");
    }

    // Компиляция нативного кода
    let mut compiler = cc::Build::new();
    compiler.file("src/native/dpdk.c");

    compiler.include("/usr/include/dpdk");
    compiler.include("/usr/include/x86_64-linux-gnu/dpdk");

    for flag in dpdk_include_path.split_whitespace() {
        if flag.starts_with("-I") {
            compiler.include(&flag[2..]);
        } else {
            compiler.flag(flag);
        }
    }

    compiler.flag("-include").flag("rte_config.h");

    // Оптимизации для production сборки
    if env::var("PROFILE").unwrap() == "release" {
        // SIMD оптимизации, если поддерживаются
        compiler.flag("-march=native");
        compiler.flag("-mtune=native");

        // Включаем агрессивную оптимизацию для скорости
        compiler.flag("-O3");
        compiler.flag("-flto");
    }

    compiler.compile("dpdk");

    println!("cargo:rerun-if-changed=src/native/dpdk.c");
    println!("cargo:rerun-if-changed=build.rs");
}

/// Проверяет, доступны ли huge pages на системе
fn check_hugepages_available() -> bool {
    Path::new("/sys/kernel/mm/hugepages").exists()
}

/// Проверяет, поддерживает ли установленная версия DPDK опции NUMA
fn check_dpdk_numa_support() -> bool {
    // Проверяем, существует ли библиотека NUMA
    Path::new("/usr/lib/libnuma.so").exists()
        || Path::new("/usr/lib64/libnuma.so").exists()
        || Path::new("/usr/lib/x86_64-linux-gnu/libnuma.so").exists()
}
