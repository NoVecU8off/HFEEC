// build.rs
use std::process::Command;

fn main() {
    // Обнаружение путей к DPDK
    let dpdk_include_path = Command::new("pkg-config")
        .args(["--cflags", "libdpdk"])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).to_string())
        .unwrap_or_else(|_| "-I/usr/local/include/dpdk".to_string());

    // Получаем флаги линковки для DPDK
    let dpdk_libs = Command::new("pkg-config")
        .args(["--libs", "libdpdk"])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).to_string())
        .unwrap_or_else(|_| {
            // Если pkg-config не возвращает библиотеки, укажем компоненты DPDK явно
            "-lrte_eal -lrte_mempool -lrte_ring -lrte_mbuf -lrte_net -lrte_ethdev".to_string()
        });

    println!("DPDK lib flags: {}", dpdk_libs);

    // Добавим пути поиска библиотек на основе известных путей к заголовочным файлам
    println!("cargo:rustc-link-search=native=/usr/lib");
    println!("cargo:rustc-link-search=native=/usr/local/lib");
    println!("cargo:rustc-link-search=native=/usr/lib/x86_64-linux-gnu");
    println!("cargo:rustc-link-search=native=/usr/lib/dpdk");
    println!("cargo:rustc-link-search=native=/usr/lib/x86_64-linux-gnu/dpdk");

    // Выводим флаги линковки для Cargo
    for lib in dpdk_libs.split_whitespace() {
        if lib.starts_with("-l") {
            // Для библиотек убираем префикс "-l"
            println!("cargo:rustc-link-lib={}", &lib[2..]);
        } else if lib.starts_with("-L") {
            // Для путей поиска библиотек убираем префикс "-L"
            println!("cargo:rustc-link-search=native={}", &lib[2..]);
        }
    }

    // Не будем добавлять dpdk как монолитную библиотеку, так как она состоит из компонентов
    // и мы уже добавили их выше

    // Компилируем C файл с правильными флагами
    let mut compiler = cc::Build::new();
    compiler.file("src/native/dpdk_helpers.c");

    // Явно добавляем пути из VSCode конфигурации
    compiler.include("/usr/include/dpdk");
    compiler.include("/usr/include/x86_64-linux-gnu/dpdk");

    // Добавляем флаги компиляции для DPDK из pkg-config
    for flag in dpdk_include_path.split_whitespace() {
        if flag.starts_with("-I") {
            compiler.include(&flag[2..]);
        } else {
            compiler.flag(flag);
        }
    }

    // Добавим флаги для поддержки DPDK
    compiler.flag("-include").flag("rte_config.h");

    compiler.compile("dpdk_helpers");

    // Указываем cargo перекомпилировать при изменении C-файлов
    println!("cargo:rerun-if-changed=src/native/dpdk_helpers.c");
}
