// build.rs - Optimized build script for HFEEC (High Frequency Electronic Exchange Connector)
use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

fn main() {
    // Set build parameters based on environment variables
    let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
    let is_release = profile == "release";
    let enable_pgo = env::var("ENABLE_PGO").unwrap_or_else(|_| "0".to_string()) == "1";
    let pgo_mode = env::var("PGO_MODE").unwrap_or_else(|_| "none".to_string());

    println!("cargo:rerun-if-env-changed=ENABLE_PGO");
    println!("cargo:rerun-if-env-changed=PGO_MODE");

    // Get DPDK paths and flags using pkg-config or fallback to defaults
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

    // Standard library search paths for DPDK
    println!("cargo:rustc-link-search=native=/usr/lib");
    println!("cargo:rustc-link-search=native=/usr/local/lib");
    println!("cargo:rustc-link-search=native=/usr/lib/x86_64-linux-gnu");
    println!("cargo:rustc-link-search=native=/usr/lib/dpdk");
    println!("cargo:rustc-link-search=native=/usr/lib/x86_64-linux-gnu/dpdk");

    // Add DPDK libraries
    for lib in dpdk_libs.split_whitespace() {
        if lib.starts_with("-l") {
            println!("cargo:rustc-link-lib={}", &lib[2..]);
        } else if lib.starts_with("-L") {
            println!("cargo:rustc-link-search=native={}", &lib[2..]);
        }
    }

    // Check if HugePages are available and enable feature flag if so
    let has_hugepages = check_hugepages_available();
    if has_hugepages {
        println!("cargo:rustc-cfg=feature=\"hugepages\"");
    }

    // Check if NUMA is supported and enable feature flag if so
    let has_numa = check_numa_support();
    if has_numa {
        println!("cargo:rustc-cfg=feature=\"numa\"");
        println!("cargo:rustc-link-lib=numa");
    }

    // Check hardware capabilities (AVX, AVX2, AVX512)
    let cpu_features = detect_cpu_features();
    for feature in cpu_features {
        println!("cargo:rustc-cfg=feature=\"{}\"", feature);
    }

    // Compile native code
    let mut compiler = cc::Build::new();
    compiler.file("src/native/dpdk.c");

    // Include DPDK headers
    compiler.include("/usr/include/dpdk");
    compiler.include("/usr/include/x86_64-linux-gnu/dpdk");

    for flag in dpdk_include_path.split_whitespace() {
        if flag.starts_with("-I") {
            compiler.include(&flag[2..]);
        } else {
            compiler.flag(flag);
        }
    }

    // Required DPDK config header
    compiler.flag("-include").flag("rte_config.h");

    // Release mode optimizations
    if is_release {
        // CPU-specific optimizations
        compiler.flag("-march=native"); // Optimize for the current CPU
        compiler.flag("-mtune=native"); // Fine-tune for the current CPU

        // Aggressive optimization flags
        compiler.flag("-O3"); // Maximum optimization level
        compiler.flag("-flto"); // Link-time optimization
        compiler.flag("-ffast-math"); // Faster but less precise floating-point
        compiler.flag("-ftree-vectorize"); // Explicitly enable vectorization
        compiler.flag("-funroll-loops"); // Unroll loops for better performance

        // Cache optimization
        compiler.flag("-fprefetch-loop-arrays"); // Prefetch data in loops

        // Add Profile-Guided Optimization if enabled
        if enable_pgo {
            match pgo_mode.as_str() {
                "generate" => {
                    // Generate profile information during test runs
                    compiler.flag("-fprofile-generate");
                    println!("PGO: Generating profile data. Run your tests now and then rebuild with PGO_MODE=use");
                }
                "use" => {
                    // Use previously generated profile information
                    let profile_dir =
                        env::var("PGO_DIR").unwrap_or_else(|_| "./pgo-data".to_string());
                    compiler.flag(&format!("-fprofile-use={}", profile_dir));
                    compiler.flag("-fprofile-correction");
                    println!("PGO: Using profile data from {}", profile_dir);
                }
                _ => {
                    println!(
                        "PGO: Mode '{}' not recognized. Valid options are 'generate' or 'use'",
                        pgo_mode
                    );
                }
            }
        }
    }

    // Additional optimization for DPDK packet processing
    compiler.define("RTE_ARCH_X86_64", None);
    compiler.define("RTE_CACHE_LINE_SIZE", Some("64"));

    // Enable thread and memory safety features
    compiler.flag("-D_FORTIFY_SOURCE=2");
    compiler.flag("-fstack-protector-strong");

    // Finally, compile the native code
    compiler.compile("dpdk");

    // Set up linker optimizations for the Rust side
    if is_release {
        println!("cargo:rustc-link-arg=-flto"); // Link-time optimization
    }

    // Trigger rebuild if native source or build script changes
    println!("cargo:rerun-if-changed=src/native/dpdk.c");
    println!("cargo:rerun-if-changed=build.rs");
}

/// Check if HugePages are available on the system
fn check_hugepages_available() -> bool {
    Path::new("/sys/kernel/mm/hugepages").exists()
}

/// Check if the system supports NUMA and has libnuma installed
fn check_numa_support() -> bool {
    // Check for libnuma library existence
    let has_libnuma = Path::new("/usr/lib/libnuma.so").exists()
        || Path::new("/usr/lib64/libnuma.so").exists()
        || Path::new("/usr/lib/x86_64-linux-gnu/libnuma.so").exists();

    if !has_libnuma {
        println!("libnuma not found, NUMA support disabled");
        return false;
    }

    // Check if the system has multiple NUMA nodes
    let has_numa_nodes = Path::new("/sys/devices/system/node/node0").exists()
        && Path::new("/sys/devices/system/node/node1").exists();

    if !has_numa_nodes {
        println!("System has only one NUMA node or NUMA not available");
    } else {
        println!("NUMA support detected with multiple nodes");
    }

    // Even with a single NUMA node, we still link with libnuma for code consistency
    true
}

/// Detect CPU features to enable specialized optimization paths
fn detect_cpu_features() -> Vec<String> {
    let mut features = Vec::new();

    // Try to read CPU info
    if let Ok(cpuinfo) = fs::read_to_string("/proc/cpuinfo") {
        // Check for common x86-64 SIMD features
        if cpuinfo.contains(" sse4_2 ") {
            features.push("sse4_2".to_string());
        }
        if cpuinfo.contains(" avx ") {
            features.push("avx".to_string());
        }
        if cpuinfo.contains(" avx2 ") {
            features.push("avx2".to_string());
        }
        if cpuinfo.contains(" avx512f ") {
            features.push("avx512".to_string());
        }

        // Check for hardware AES support
        if cpuinfo.contains(" aes ") {
            features.push("aes_ni".to_string());
        }

        // Check for other useful features
        if cpuinfo.contains(" rdrand ") {
            features.push("rdrand".to_string());
        }
        if cpuinfo.contains(" rdseed ") {
            features.push("rdseed".to_string());
        }
    }

    println!("Detected CPU features: {:?}", features);
    features
}
