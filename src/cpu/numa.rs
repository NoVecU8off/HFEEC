// src/cpu/numa.rs - NUMA-aware topology management
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;

use core_affinity::CoreId;

use super::topology::CpuTopology;

#[derive(Debug, Clone)]
pub struct NumaTopology {
    /// Number of NUMA nodes in the system
    pub num_nodes: usize,
    /// Mapping of NUMA node ID to list of physical cores on that node
    pub node_cores: HashMap<usize, Vec<usize>>,
    /// Mapping of NUMA node ID to list of memory regions
    pub node_memory: HashMap<usize, Vec<String>>,
    /// Mapping of PCI devices to NUMA nodes
    pub device_node: HashMap<String, usize>,
    /// Mapping of network interfaces to NUMA nodes
    pub nic_node: HashMap<String, usize>,
}

impl NumaTopology {
    pub fn new() -> io::Result<Self> {
        let mut topology = NumaTopology {
            num_nodes: 0,
            node_cores: HashMap::new(),
            node_memory: HashMap::new(),
            device_node: HashMap::new(),
            nic_node: HashMap::new(),
        };

        topology.load_topology()?;
        Ok(topology)
    }

    /// Loads NUMA topology information from system files
    fn load_topology(&mut self) -> io::Result<()> {
        let numa_path = Path::new("/sys/devices/system/node");
        if !numa_path.exists() {
            // System does not support NUMA or has only one node
            self.num_nodes = 1;
            return Ok(());
        }

        // Find all available NUMA nodes
        let mut nodes = HashSet::new();
        for entry in fs::read_dir(numa_path)? {
            let entry = entry?;
            let path = entry.path();
            let filename = path.file_name().unwrap().to_string_lossy();

            if !filename.starts_with("node") || !filename[4..].chars().all(char::is_numeric) {
                continue;
            }

            let node_id: usize = filename[4..].parse().unwrap_or(0);
            nodes.insert(node_id);

            // Load CPU information for this node
            self.load_node_cpus(node_id, &path)?;

            // Load memory information for this node
            self.load_node_memory(node_id, &path)?;
        }

        self.num_nodes = nodes.len();

        // Load PCI to NUMA mapping
        self.load_pci_numa_mapping()?;

        Ok(())
    }

    /// Loads CPU information for a specific NUMA node
    fn load_node_cpus(&mut self, node_id: usize, node_path: &Path) -> io::Result<()> {
        let cpulist_path = node_path.join("cpulist");
        if !cpulist_path.exists() {
            return Ok(());
        }

        let cpulist = read_first_line(&cpulist_path)?;
        let cpu_ids = parse_cpu_list(&cpulist);

        self.node_cores.insert(node_id, cpu_ids);
        Ok(())
    }

    /// Loads memory information for a specific NUMA node
    fn load_node_memory(&mut self, node_id: usize, node_path: &Path) -> io::Result<()> {
        let meminfo_path = node_path.join("meminfo");
        if !meminfo_path.exists() {
            return Ok(());
        }

        let meminfo = read_file_contents(&meminfo_path)?;
        let mem_regions = meminfo
            .lines()
            .filter(|line| line.contains("MemTotal"))
            .map(|line| line.to_string())
            .collect::<Vec<String>>();

        self.node_memory.insert(node_id, mem_regions);
        Ok(())
    }

    /// Loads PCI to NUMA node mapping
    fn load_pci_numa_mapping(&mut self) -> io::Result<()> {
        // Iterate through PCI devices
        let pci_path = Path::new("/sys/bus/pci/devices");
        if !pci_path.exists() {
            return Ok(());
        }

        for entry in fs::read_dir(pci_path)? {
            let entry = entry?;
            let path = entry.path();
            let device_id = path.file_name().unwrap().to_string_lossy().to_string();

            // Check if this is a network device
            let is_network = path.join("class").exists() && {
                let class = read_first_line(&path.join("class"))?;
                class.starts_with("0x02") // 0x02 is the network class
            };

            if !is_network {
                continue;
            }

            // Try to find the NUMA node for this device
            let numa_node_path = path.join("numa_node");
            if numa_node_path.exists() {
                if let Ok(node_str) = read_first_line(&numa_node_path) {
                    if let Ok(node_id) = node_str.trim().parse::<i32>() {
                        if node_id >= 0 {
                            self.device_node.insert(device_id.clone(), node_id as usize);
                        }
                    }
                }
            }

            // Get network interface name if available
            if let Ok(netdev_dirs) = fs::read_dir(path.join("net")) {
                for netdev_entry in netdev_dirs {
                    if let Ok(netdev_entry) = netdev_entry {
                        let ifname = netdev_entry.file_name().to_string_lossy().to_string();
                        // Check if we know the NUMA node for this device
                        if let Some(&node_id) = self.device_node.get(&device_id) {
                            self.nic_node.insert(ifname, node_id);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Returns the NUMA node ID for a given network interface name
    pub fn get_nic_node(&self, ifname: &str) -> Option<usize> {
        self.nic_node.get(ifname).copied()
    }

    /// Returns all physical core IDs on a specific NUMA node, excluding hyperthread cores and core 0
    pub fn get_node_physical_cores(
        &self,
        node_id: usize,
        cpu_topology: &CpuTopology,
    ) -> Vec<usize> {
        if let Some(core_ids) = self.node_cores.get(&node_id) {
            let physical_cores = cpu_topology.get_physical_core_ids();
            core_ids
                .iter()
                .filter(|&&id| physical_cores.contains(&id) && id != 0)
                .copied()
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Returns all core_affinity::CoreId objects for a specific NUMA node,
    /// excluding hyperthread cores and core 0
    pub fn get_node_core_ids(&self, node_id: usize, cpu_topology: &CpuTopology) -> Vec<CoreId> {
        self.get_node_physical_cores(node_id, cpu_topology)
            .into_iter()
            .map(|id| CoreId { id })
            .collect()
    }

    /// Provides the recommended socket memory configuration for DPDK based on NUMA topology
    pub fn get_socket_memory_config(&self, mb_per_node: usize) -> Vec<String> {
        let mut socket_mem = Vec::new();

        if self.num_nodes <= 1 {
            // Single NUMA node or non-NUMA system
            socket_mem.push(format!("--socket-mem={}", mb_per_node));
        } else {
            // Multi-NUMA system: allocate memory for each node
            let socket_mem_values = (0..self.num_nodes)
                .map(|_| mb_per_node.to_string())
                .collect::<Vec<String>>()
                .join(",");

            socket_mem.push(format!("--socket-mem={}", socket_mem_values));
        }

        socket_mem
    }

    /// Prints NUMA topology information for debugging
    pub fn print_topology_info(&self, cpu_topology: &CpuTopology) {
        println!("NUMA Topology Information:");
        println!("  NUMA nodes: {}", self.num_nodes);

        for node_id in 0..self.num_nodes {
            println!("\nNUMA Node {}:", node_id);

            // Print cores for this node
            if let Some(cores) = self.node_cores.get(&node_id) {
                println!("  All logical cores: {:?}", cores);

                // Get physical cores (non-HT) for this node
                let physical_cores = self.get_node_physical_cores(node_id, cpu_topology);
                println!("  Physical cores (excluding core 0): {:?}", physical_cores);
            } else {
                println!("  No cores found");
            }

            // Print memory info for this node
            if let Some(mem_info) = self.node_memory.get(&node_id) {
                for mem_line in mem_info {
                    println!("  Memory: {}", mem_line);
                }
            }

            // Print network devices on this node
            println!("  Network interfaces:");
            let mut found = false;
            for (ifname, &if_node) in &self.nic_node {
                if if_node == node_id {
                    println!("    {}", ifname);
                    found = true;
                }
            }

            if !found {
                println!("    None found");
            }
        }
    }
}

/// Reads the first line from a file
fn read_first_line<P: AsRef<Path>>(path: P) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;

    Ok(contents.lines().next().unwrap_or("").to_string())
}

/// Reads entire file contents
fn read_file_contents<P: AsRef<Path>>(path: P) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;

    Ok(contents)
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
