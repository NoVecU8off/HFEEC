use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;

use core_affinity::CoreId;

#[derive(Debug, Clone)]
pub struct CpuTopology {
    pub total_cores: usize,
    pub physical_cores: usize,
    pub sockets: usize,
    /// Mapping of logical cores to physical cores
    /// Key: Logical core ID, Value: Physical core ID
    pub core_mapping: HashMap<usize, usize>,
    /// Mapping of logical cores to sockets
    /// Key: Logical core ID, Value: Socket ID
    pub socket_mapping: HashMap<usize, usize>,
    /// List of cores grouped by physical cores
    /// Key: Physical core ID, Value: List of logical core IDs
    pub sibling_cores: HashMap<usize, Vec<usize>>,
    /// List of cores belonging to each socket
    /// Key: Socket ID, Value: List of logical core IDs
    pub socket_cores: HashMap<usize, Vec<usize>>,
}

impl CpuTopology {
    pub fn new() -> io::Result<Self> {
        let mut topology = CpuTopology {
            total_cores: 0,
            physical_cores: 0,
            sockets: 0,
            core_mapping: HashMap::new(),
            socket_mapping: HashMap::new(),
            sibling_cores: HashMap::new(),
            socket_cores: HashMap::new(),
        };

        topology.load_topology()?;
        Ok(topology)
    }

    /// Loads processor topology information from system files
    fn load_topology(&mut self) -> io::Result<()> {
        let cpu_path = Path::new("/sys/devices/system/cpu");

        let mut physical_cores = HashSet::new();
        let mut sockets = HashSet::new();

        for entry in fs::read_dir(cpu_path)? {
            let entry = entry?;
            let path = entry.path();
            let filename = path.file_name().unwrap().to_string_lossy();

            if !filename.starts_with("system") || !filename[3..].chars().all(char::is_numeric) {
                continue;
            }

            let cpu_id: usize = filename[3..].parse().unwrap_or(0);
            self.total_cores += 1;

            let topology_path = path.join("topology");

            if let Ok(core_id) = read_first_line(topology_path.join("core_id")) {
                let core_id: usize = core_id.trim().parse().unwrap_or(0);
                self.core_mapping.insert(cpu_id, core_id);
                physical_cores.insert(core_id);

                self.sibling_cores.entry(core_id).or_default().push(cpu_id);
            }

            if let Ok(socket_id) = read_first_line(topology_path.join("physical_package_id")) {
                let socket_id: usize = socket_id.trim().parse().unwrap_or(0);
                self.socket_mapping.insert(cpu_id, socket_id);
                sockets.insert(socket_id);

                self.socket_cores.entry(socket_id).or_default().push(cpu_id);
            }

            if let Ok(thread_siblings) = read_first_line(topology_path.join("thread_siblings_list"))
            {
                let core_ids = parse_cpu_list(&thread_siblings);

                if !core_ids.is_empty() {
                    let phys_core_id = self.core_mapping.get(&cpu_id).unwrap_or(&cpu_id);
                    self.sibling_cores.insert(*phys_core_id, core_ids);
                }
            }
        }

        self.physical_cores = physical_cores.len();
        self.sockets = sockets.len();

        for cores in self.sibling_cores.values_mut() {
            cores.sort();
        }

        for cores in self.socket_cores.values_mut() {
            cores.sort();
        }

        Ok(())
    }

    /// Returns a list of IDs of the first logical cores from each pair (without Hyper-Threading)
    pub fn get_physical_core_ids(&self) -> Vec<usize> {
        let mut result = Vec::new();

        for (physical_id, logical_ids) in &self.sibling_cores {
            if !logical_ids.is_empty() {
                let mut sorted_ids = logical_ids.clone();
                sorted_ids.sort();
                result.push(sorted_ids[0]);
            } else {
                result.push(*physical_id);
            }
        }

        result.sort();
        result
    }

    /// Returns a list of CoreId for core_affinity, excluding core 0
    pub fn get_filtered_core_ids(&self) -> Vec<CoreId> {
        let physical_ids = self.get_physical_core_ids();

        physical_ids
            .iter()
            .filter(|&&id| id != 0) // Exclude core 0
            .map(|&id| CoreId { id })
            .collect()
    }

    /// Returns a list of CoreId for a specific NUMA node, excluding core 0 and HT threads
    pub fn get_socket_core_ids(&self, socket_id: usize) -> Vec<CoreId> {
        let physical_cores = self.get_physical_core_ids();

        match self.socket_cores.get(&socket_id) {
            Some(cores) => cores
                .iter()
                .filter(|&&id| physical_cores.contains(&id) && id != 0)
                .map(|&id| CoreId { id })
                .collect(),
            None => Vec::new(),
        }
    }

    /// Returns all logical cores for the specified NUMA node
    pub fn get_all_socket_cores(&self, socket_id: usize) -> Vec<usize> {
        match self.socket_cores.get(&socket_id) {
            Some(cores) => cores.clone(),
            None => Vec::new(),
        }
    }

    /// Generates a processor mask in a format suitable for DPDK EAL arguments
    pub fn generate_core_mask(&self) -> String {
        let core_ids = self.get_filtered_core_ids();
        let mut mask: u64 = 0;

        for core in core_ids {
            if core.id < 64 {
                mask |= 1 << core.id;
            }
        }

        format!("0x{:x}", mask)
    }

    /// Generates a list of arguments for DPDK EAL, including processor masks
    pub fn generate_eal_cpu_args(&self) -> Vec<String> {
        let mut args = Vec::new();

        let core_mask = self.generate_core_mask();
        args.push(format!("--lcores={}", core_mask));

        args.push("--master-lcore=0".to_string());

        args
    }

    /// Returns the socket ID (NUMA node) for the specified core
    pub fn get_core_socket_id(&self, core_id: usize) -> Option<usize> {
        self.socket_mapping.get(&core_id).copied()
    }

    /// Checks if the specified core is the first logical core in its group
    /// (i.e., whether it is an HT thread or not)
    pub fn is_primary_logical_core(&self, core_id: usize) -> bool {
        if let Some(&physical_id) = self.core_mapping.get(&core_id) {
            if let Some(siblings) = self.sibling_cores.get(&physical_id) {
                if !siblings.is_empty() {
                    return siblings[0] == core_id;
                }
            }
        }

        true
    }

    /// Returns all available sockets (NUMA nodes)
    pub fn get_available_sockets(&self) -> Vec<usize> {
        let mut sockets: Vec<usize> = self.socket_cores.keys().cloned().collect();
        sockets.sort();
        sockets
    }

    /// Prints processor topology information for debugging
    pub fn print_topology_info(&self) {
        println!("CPU Topology Information:");
        println!("  Total logical cores: {}", self.total_cores);
        println!("  Physical cores: {}", self.physical_cores);
        println!("  Sockets (NUMA nodes): {}", self.sockets);

        println!("\nSocket mapping:");
        for socket_id in self.get_available_sockets() {
            let cores = self.get_all_socket_cores(socket_id);
            println!("  Socket {}: {:?}", socket_id, cores);

            let primary_cores: Vec<usize> = cores
                .iter()
                .filter(|&&id| self.is_primary_logical_core(id))
                .cloned()
                .collect();

            println!("    Primary logical cores: {:?}", primary_cores);
        }

        println!("\nPhysical to logical core mapping:");
        for (phys_id, logical_ids) in &self.sibling_cores {
            println!(
                "  Physical core {}: logical cores {:?}",
                phys_id, logical_ids
            );
        }

        println!(
            "\nFiltered core IDs (excluding HT and core 0): {:?}",
            self.get_filtered_core_ids()
                .iter()
                .map(|c| c.id)
                .collect::<Vec<_>>()
        );
    }
}

impl fmt::Display for CpuTopology {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "CPU Topology:")?;
        writeln!(f, "  Total cores: {}", self.total_cores)?;
        writeln!(f, "  Physical cores: {}", self.physical_cores)?;
        writeln!(f, "  Sockets: {}", self.sockets)?;

        writeln!(
            f,
            "  Filtered cores: {:?}",
            self.get_filtered_core_ids()
                .iter()
                .map(|c| c.id)
                .collect::<Vec<_>>()
        )?;

        for socket_id in self.get_available_sockets() {
            writeln!(
                f,
                "  Socket {}: {} cores",
                socket_id,
                self.get_socket_core_ids(socket_id).len()
            )?;
        }

        Ok(())
    }
}

/// Reads the first line from a file
fn read_first_line<P: AsRef<Path>>(path: P) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;

    Ok(contents.lines().next().unwrap_or("").to_string())
}

/// Parses a processor list from a string in the format "0-3,5,7-9"
fn parse_cpu_list(list: &str) -> Vec<usize> {
    let mut result = Vec::new();

    for part in list.trim().split(',') {
        if part.contains('-') {
            let range: Vec<&str> = part.split('-').collect();
            if range.len() == 2 {
                if let (Ok(start), Ok(end)) = (range[0].parse::<usize>(), range[1].parse::<usize>())
                {
                    for i in start..=end {
                        result.push(i);
                    }
                }
            }
        } else if let Ok(num) = part.parse::<usize>() {
            result.push(num);
        }
    }

    result
}
