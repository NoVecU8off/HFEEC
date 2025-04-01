use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct HugePagesInfo {
    pub size_2mb_available: u32,
    pub size_1gb_available: u32,
    pub size_2mb_total: u32,
    pub size_1gb_total: u32,
    pub numa_mapping: Vec<(u32, u32, u32)>,
}

pub fn check_hugepages_available() -> bool {
    Path::new("/sys/kernel/mm/hugepages").exists()
}

pub fn get_hugepages_info() -> io::Result<HugePagesInfo> {
    let mut info = HugePagesInfo {
        size_2mb_available: 0,
        size_1gb_available: 0,
        size_2mb_total: 0,
        size_1gb_total: 0,
        numa_mapping: Vec::new(),
    };

    if let Ok(mut file) = File::open("/sys/kernel/mm/hugepages/hugepages-2048kB/nr_hugepages") {
        let mut content = String::new();
        file.read_to_string(&mut content)?;
        info.size_2mb_total = content.trim().parse().unwrap_or(0);
    }

    if let Ok(mut file) = File::open("/sys/kernel/mm/hugepages/hugepages-2048kB/free_hugepages") {
        let mut content = String::new();
        file.read_to_string(&mut content)?;
        info.size_2mb_available = content.trim().parse().unwrap_or(0);
    }

    if Path::new("/sys/kernel/mm/hugepages/hugepages-1048576kB").exists() {
        if let Ok(mut file) =
            File::open("/sys/kernel/mm/hugepages/hugepages-1048576kB/nr_hugepages")
        {
            let mut content = String::new();
            file.read_to_string(&mut content)?;
            info.size_1gb_total = content.trim().parse().unwrap_or(0);
        }

        if let Ok(mut file) =
            File::open("/sys/kernel/mm/hugepages/hugepages-1048576kB/free_hugepages")
        {
            let mut content = String::new();
            file.read_to_string(&mut content)?;
            info.size_1gb_available = content.trim().parse().unwrap_or(0);
        }
    }

    if Path::new("/sys/devices/system/node").exists() {
        if let Ok(entries) = fs::read_dir("/sys/devices/system/node") {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(name) = path.file_name() {
                    if let Some(name_str) = name.to_str() {
                        if name_str.starts_with("node") {
                            let node_id: u32 = name_str[4..].parse().unwrap_or(0);

                            let mut node_2mb = 0;
                            let mut node_1gb = 0;

                            let path_2mb = path.join("hugepages/hugepages-2048kB/nr_hugepages");
                            if path_2mb.exists() {
                                if let Ok(mut file) = File::open(&path_2mb) {
                                    let mut content = String::new();
                                    file.read_to_string(&mut content)?;
                                    node_2mb = content.trim().parse().unwrap_or(0);
                                }
                            }

                            let path_1gb = path.join("hugepages/hugepages-1048576kB/nr_hugepages");
                            if path_1gb.exists() {
                                if let Ok(mut file) = File::open(&path_1gb) {
                                    let mut content = String::new();
                                    file.read_to_string(&mut content)?;
                                    node_1gb = content.trim().parse().unwrap_or(0);
                                }
                            }

                            info.numa_mapping.push((node_id, node_2mb, node_1gb));
                        }
                    }
                }
            }
        }
    }

    Ok(info)
}

pub fn configure_hugepages(mb_2m_count: u32, mb_1g_count: u32) -> io::Result<()> {
    if mb_2m_count > 0 {
        let output = Command::new("sudo")
            .args(["sysctl", "-w", &format!("vm.nr_hugepages={}", mb_2m_count)])
            .output()?;

        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to configure 2MB hugepages: {}", error),
            ));
        }
    }

    if mb_1g_count > 0 && Path::new("/sys/kernel/mm/hugepages/hugepages-1048576kB").exists() {
        let output = Command::new("sudo")
            .args([
                "sysctl",
                "-w",
                &format!("vm.nr_hugepages_1g={}", mb_1g_count),
            ])
            .output()?;

        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to configure 1GB hugepages: {}", error),
            ));
        }
    }

    Ok(())
}

pub fn mount_hugetlbfs(mount_path: &str, page_size: &str) -> io::Result<()> {
    if !Path::new(mount_path).exists() {
        fs::create_dir_all(mount_path)?;
    }

    let output = Command::new("mount").output()?;

    let mount_output = String::from_utf8_lossy(&output.stdout);
    if mount_output.contains(mount_path) && mount_output.contains("hugetlbfs") {
        return Ok(());
    }

    let output = Command::new("sudo")
        .args([
            "mount",
            "-t",
            "hugetlbfs",
            "-o",
            &format!("pagesize={}", page_size),
            "none",
            mount_path,
        ])
        .output()?;

    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("Failed to mount hugetlbfs: {}", error),
        ));
    }

    Ok(())
}

pub fn recommend_hugepage_config() -> io::Result<(u32, u32, Vec<String>)> {
    let num_numa_nodes = get_numa_node_count()?;
    let total_memory_mb = get_total_memory_mb()?;

    let total_hugepage_memory = total_memory_mb / 2;

    let mut pages_2mb = 0;
    let mut pages_1gb = 0;
    let mut eal_args = Vec::new();

    if total_memory_mb > 16 * 1024 {
        pages_1gb = total_hugepage_memory / 1024;
    } else {
        pages_2mb = total_hugepage_memory / 2;
    }

    if num_numa_nodes > 1 {
        let mem_per_node = total_hugepage_memory / num_numa_nodes;
        let socket_mem = (0..num_numa_nodes)
            .map(|_| mem_per_node.to_string())
            .collect::<Vec<_>>()
            .join(",");
        eal_args.push(format!("--socket-mem={}", socket_mem));
    } else {
        eal_args.push(format!("--socket-mem={}", total_hugepage_memory));
    }

    eal_args.push("--huge-unlink".to_string());

    Ok((pages_2mb, pages_1gb, eal_args))
}

fn get_numa_node_count() -> io::Result<u32> {
    if !Path::new("/sys/devices/system/node").exists() {
        return Ok(1);
    }

    let mut count = 0;
    for entry in fs::read_dir("/sys/devices/system/node")? {
        let entry = entry?;
        let path = entry.path();
        if let Some(name) = path.file_name() {
            if let Some(name_str) = name.to_str() {
                if name_str.starts_with("node") {
                    count += 1;
                }
            }
        }
    }

    Ok(count)
}

fn get_total_memory_mb() -> io::Result<u32> {
    let mut file = File::open("/proc/meminfo")?;
    let mut content = String::new();
    file.read_to_string(&mut content)?;

    for line in content.lines() {
        if line.starts_with("MemTotal:") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                if let Ok(kb) = parts[1].parse::<u32>() {
                    return Ok(kb / 1024);
                }
            }
        }
    }

    Err(io::Error::new(
        io::ErrorKind::Other,
        "Failed to get total memory",
    ))
}
